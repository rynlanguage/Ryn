//! Text encoding of the typed IR that semantic analysis produces.
//!
//! This is the interchange format for a Ryn-written middle end and backend: the
//! IR can be written out after `sema` and read back without the Rust analyzer.
//! It follows the conventions of `ast_codec`: a prefix-order stream of words,
//! each preceded by one space; strings are `<byte length>:<bytes>`; spans are
//! two integers; an optional value is `-` or `+` followed by the value; a list
//! is its length followed by the items; floats are their IEEE bit pattern.
//!
//! Types are written by content, never by registry index. An array is written
//! as its element and length, a function pointer as its signature, and so on.
//! Decoding interns those again, so `encode(decode(encode(ir)))` equals
//! `encode(ir)` even though the process-local registry indices may differ.

use std::fmt::Write as _;

use crate::{
    ast::BinaryOp,
    filesystem_ops::FilesystemOp,
    map_ops::MapOp,
    sema::{
        EnumPredicate, FunctionPointerSignature, IrCallTarget, IrExpression, IrPrintPart,
        IrStatement, LocalBinding, LocalType, RynArmBinding, RynEnum, RynEnumVariant, RynFunction,
        RynIr, RynMatchArm, RynStruct, RynStructField, Type, array_info, function_pointer_info,
        intern_array, intern_function_pointer, intern_map, intern_pointer_target, intern_vec_elem,
        map_info, pointer_target, vec_elem,
    },
    source::Span,
    string_ops::StringOp,
    system_ops::SystemOp,
    vector_ops::VecOp,
};

/// Encodes a complete IR as `Ir <program>`.
pub fn encode_ir(ir: &RynIr) -> String {
    let mut encoder = Encoder::default();
    encoder.word("Ir");
    encoder.ir(ir);
    encoder.out
}

/// Decodes the output of [`encode_ir`].
pub fn decode_ir(text: &str) -> Result<RynIr, String> {
    let mut decoder = Decoder { text, at: 0 };
    decoder.expect_word("Ir")?;
    let ir = decoder.ir()?;
    decoder.finish()?;
    Ok(ir)
}

#[derive(Default)]
struct Encoder {
    out: String,
}

impl Encoder {
    fn word(&mut self, word: &str) {
        self.out.push(' ');
        self.out.push_str(word);
    }

    fn uint(&mut self, value: u64) {
        let _ = write!(self.out, " {value}");
    }

    fn int(&mut self, value: i128) {
        let _ = write!(self.out, " {value}");
    }

    fn boolean(&mut self, value: bool) {
        self.uint(u64::from(value));
    }

    fn string(&mut self, value: &str) {
        let _ = write!(self.out, " {}:{value}", value.len());
    }

    fn span(&mut self, span: Span) {
        self.uint(span.start as u64);
        self.uint(span.end as u64);
    }

    fn optional_uint(&mut self, value: Option<usize>) {
        match value {
            None => self.word("-"),
            Some(value) => {
                self.word("+");
                self.uint(value as u64);
            }
        }
    }

    fn optional_type(&mut self, value: Option<Type>) {
        match value {
            None => self.word("-"),
            Some(value) => {
                self.word("+");
                self.ty(value);
            }
        }
    }

    fn list_len(&mut self, length: usize) {
        self.uint(length as u64);
    }

    fn ir(&mut self, ir: &RynIr) {
        self.uint(ir.main_index as u64);
        self.optional_uint(ir.instant_drop);
        self.list_len(ir.structs.len());
        for definition in &ir.structs {
            self.struct_definition(definition);
        }
        self.list_len(ir.enums.len());
        for definition in &ir.enums {
            self.enum_definition(definition);
        }
        self.list_len(ir.functions.len());
        for function in &ir.functions {
            self.function(function);
        }
    }

    fn struct_definition(&mut self, definition: &RynStruct) {
        self.string(&definition.name);
        self.list_len(definition.fields.len());
        for field in &definition.fields {
            self.string(&field.name);
            self.ty(field.ty);
            self.uint(field.slot_offset as u64);
            self.boolean(field.public);
        }
        self.uint(definition.slot_count as u64);
        self.boolean(definition.repr_c);
        self.optional_uint(definition.drop_function);
        self.boolean(definition.derives_clone);
        self.boolean(definition.derives_hash);
        self.string(&definition.module_path);
    }

    fn enum_definition(&mut self, definition: &RynEnum) {
        self.string(&definition.name);
        self.list_len(definition.variants.len());
        for variant in &definition.variants {
            self.string(&variant.name);
            self.list_len(variant.fields.len());
            for field in &variant.fields {
                self.ty(*field);
            }
        }
    }

    fn function(&mut self, function: &RynFunction) {
        self.list_len(function.parameters.len());
        for parameter in &function.parameters {
            self.uint(parameter.slot as u64);
            self.ty(parameter.ty);
        }
        match &function.external_symbol {
            None => self.word("-"),
            Some(symbol) => {
                self.word("+");
                self.string(symbol);
            }
        }
        self.optional_type(function.return_type);
        self.optional_uint(function.reference_return_parameter);
        match &function.return_value {
            None => self.word("-"),
            Some(value) => {
                self.word("+");
                self.expression(value);
            }
        }
        self.statements(&function.statements);
        self.list_len(function.local_types.len());
        for local in &function.local_types {
            self.local_type(*local);
        }
        self.list_len(function.owned_slot_types.len());
        for slot in &function.owned_slot_types {
            self.optional_type(*slot);
        }
        self.list_len(function.addressed_slot_types.len());
        for slot in &function.addressed_slot_types {
            self.optional_type(*slot);
        }
    }

    fn local_type(&mut self, local: LocalType) {
        self.word(match local {
            LocalType::I8 => "I8",
            LocalType::I16 => "I16",
            LocalType::I32 => "I32",
            LocalType::I64 => "I64",
            LocalType::F32 => "F32",
            LocalType::F64 => "F64",
            LocalType::Ptr => "Ptr",
            LocalType::OwnedPtr => "OwnedPtr",
        });
    }

    fn ty(&mut self, ty: Type) {
        match ty {
            Type::I8 => self.word("I8"),
            Type::I16 => self.word("I16"),
            Type::I32 => self.word("I32"),
            Type::I64 => self.word("I64"),
            Type::U8 => self.word("U8"),
            Type::U16 => self.word("U16"),
            Type::U32 => self.word("U32"),
            Type::U64 => self.word("U64"),
            Type::F32 => self.word("F32"),
            Type::F64 => self.word("F64"),
            Type::Str => self.word("Str"),
            Type::OwnedString => self.word("OwnedString"),
            Type::Char => self.word("Char"),
            Type::Bool => self.word("Bool"),
            Type::Struct(id) => {
                self.word("Struct");
                self.uint(id as u64);
            }
            Type::Enum(id) => {
                self.word("Enum");
                self.uint(id as u64);
            }
            Type::Vec(id) => {
                self.word("Vec");
                self.ty(vec_elem(id));
            }
            Type::Slice(id) => {
                self.word("Slice");
                self.ty(vec_elem(id));
            }
            Type::Array(id) => {
                let (element, length) = array_info(id);
                self.word("Array");
                self.ty(element);
                self.uint(length as u64);
            }
            Type::Map(id) => {
                let (key, value) = map_info(id);
                self.word("Map");
                self.ty(key);
                self.ty(value);
            }
            Type::Set(id) => {
                let (key, value) = map_info(id);
                self.word("Set");
                self.ty(key);
                self.ty(value);
            }
            Type::Reference(id, mutable) => {
                self.word("Reference");
                self.ty(pointer_target(id));
                self.boolean(mutable);
            }
            Type::RawPointer(id) => {
                self.word("RawPointer");
                self.ty(pointer_target(id));
            }
            Type::FunctionPointer(id) => {
                let signature = function_pointer_info(id);
                self.word("FunctionPointer");
                self.list_len(signature.parameters.len());
                for parameter in &signature.parameters {
                    self.ty(*parameter);
                }
                self.optional_type(signature.result);
                self.boolean(signature.extern_c);
            }
        }
    }

    fn statements(&mut self, statements: &[IrStatement]) {
        self.list_len(statements.len());
        for statement in statements {
            self.statement(statement);
        }
    }

    fn expressions(&mut self, expressions: &[IrExpression]) {
        self.list_len(expressions.len());
        for expression in expressions {
            self.expression(expression);
        }
    }

    fn statement(&mut self, statement: &IrStatement) {
        match statement {
            IrStatement::Block(statements) => {
                self.word("Block");
                self.statements(statements);
            }
            IrStatement::Drop { slots } => {
                self.word("Drop");
                self.list_len(slots.len());
                for (slot, ty) in slots {
                    self.uint(*slot as u64);
                    self.ty(*ty);
                }
            }
            IrStatement::Let {
                slot,
                ty,
                value,
                span,
            } => {
                self.word("Let");
                self.uint(*slot as u64);
                self.ty(*ty);
                self.expression(value);
                self.span(*span);
            }
            IrStatement::Assign {
                slot,
                ty,
                value,
                span,
            } => {
                self.word("Assign");
                self.uint(*slot as u64);
                self.ty(*ty);
                self.expression(value);
                self.span(*span);
            }
            IrStatement::ArrayAssign {
                slot,
                element,
                length,
                index,
                value,
                span,
            } => {
                self.word("ArrayAssign");
                self.uint(*slot as u64);
                self.ty(*element);
                self.uint(*length as u64);
                self.expression(index);
                self.expression(value);
                self.span(*span);
            }
            IrStatement::DereferenceAssign { pointer, ty, value } => {
                self.word("DereferenceAssign");
                self.expression(pointer);
                self.ty(*ty);
                self.expression(value);
            }
            IrStatement::FieldAssign { slot, ty, value } => {
                self.word("FieldAssign");
                self.uint(*slot as u64);
                self.ty(*ty);
                self.expression(value);
            }
            IrStatement::ReferenceFieldAssign {
                pointer,
                struct_id,
                field_index,
                ty,
                value,
            } => {
                self.word("ReferenceFieldAssign");
                self.expression(pointer);
                self.uint(*struct_id as u64);
                self.uint(*field_index as u64);
                self.ty(*ty);
                self.expression(value);
            }
            IrStatement::Print { value, ty } => {
                self.word("Print");
                self.expression(value);
                self.ty(*ty);
            }
            IrStatement::PrintTemplate(parts) => {
                self.word("PrintTemplate");
                self.list_len(parts.len());
                for part in parts {
                    self.print_part(part);
                }
            }
            IrStatement::Call { target, arguments } => {
                self.word("CallStatement");
                self.call_target(target);
                self.expressions(arguments);
            }
            IrStatement::If {
                condition,
                then_body,
                else_body,
            } => {
                self.word("IfStatement");
                self.expression(condition);
                self.statements(then_body);
                self.statements(else_body);
            }
            IrStatement::While {
                setup,
                condition,
                body,
            } => {
                self.word("While");
                self.statements(setup);
                self.expression(condition);
                self.statements(body);
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
                self.word("For");
                self.uint(*slot as u64);
                self.uint(*end_slot as u64);
                self.ty(*ty);
                self.expression(start);
                self.expression(end);
                self.boolean(*inclusive);
                self.statements(body);
            }
            IrStatement::Break => self.word("Break"),
            IrStatement::Continue => self.word("Continue"),
            IrStatement::Return { value } => {
                self.word("Return");
                match value {
                    None => self.word("-"),
                    Some(value) => {
                        self.word("+");
                        self.expression(value);
                    }
                }
            }
        }
    }

    fn print_part(&mut self, part: &IrPrintPart) {
        match part {
            IrPrintPart::Text(text) => {
                self.word("Text");
                self.string(text);
            }
            IrPrintPart::Value { value, ty } => {
                self.word("Value");
                self.expression(value);
                self.ty(*ty);
            }
        }
    }

    fn call_target(&mut self, target: &IrCallTarget) {
        match target {
            IrCallTarget::Function(id) => {
                self.word("Function");
                self.uint(*id as u64);
            }
            IrCallTarget::ArgumentCount => self.word("ArgumentCount"),
            IrCallTarget::Argument => self.word("Argument"),
            IrCallTarget::String(op) => {
                self.word("String");
                self.word(string_op_name(*op));
            }
            IrCallTarget::Filesystem(op) => {
                self.word("Filesystem");
                self.word(filesystem_op_name(*op));
            }
            IrCallTarget::TryReadFile(id) => {
                self.word("TryReadFile");
                self.uint(*id as u64);
            }
            IrCallTarget::ReadFileResult(id) => {
                self.word("ReadFileResult");
                self.uint(*id as u64);
            }
            IrCallTarget::System(op) => {
                self.word("System");
                self.word(system_op_name(*op));
            }
            IrCallTarget::IndirectFunctionPointer(id) => {
                self.word("IndirectFunctionPointer");
                self.uint(*id as u64);
            }
            IrCallTarget::Vec(op, id) => {
                self.word("Vec");
                self.word(vec_op_name(*op));
                self.uint(*id as u64);
            }
            IrCallTarget::VecSlice(id) => {
                self.word("VecSlice");
                self.uint(*id as u64);
            }
            IrCallTarget::SliceLen => self.word("SliceLen"),
            IrCallTarget::Map(op, id) => {
                self.word("Map");
                self.word(map_op_name(*op));
                self.uint(*id as u64);
            }
            IrCallTarget::Set(op, id) => {
                self.word("Set");
                self.word(map_op_name(*op));
                self.uint(*id as u64);
            }
            IrCallTarget::EnumNew { enum_id, tag } => {
                self.word("EnumNew");
                self.uint(*enum_id as u64);
                self.uint(*tag as u64);
            }
            IrCallTarget::EnumPredicate(predicate, enum_id) => {
                self.word("EnumPredicate");
                self.word(enum_predicate_name(*predicate));
                self.uint(*enum_id as u64);
            }
            IrCallTarget::EnumUnwrapOr {
                enum_id,
                value_type,
            } => {
                self.word("EnumUnwrapOr");
                self.uint(*enum_id as u64);
                self.ty(*value_type);
            }
            IrCallTarget::EnumUnwrap {
                enum_id,
                value_type,
                message,
                success_tag,
                failure,
            } => {
                self.word("EnumUnwrap");
                self.uint(*enum_id as u64);
                self.ty(*value_type);
                self.boolean(*message);
                self.uint(*success_tag as u64);
                self.word(system_op_name(*failure));
            }
            IrCallTarget::VecGetOption {
                elem_id,
                option_id,
                pop,
            } => {
                self.word("VecGetOption");
                self.uint(*elem_id as u64);
                self.uint(*option_id as u64);
                self.boolean(*pop);
            }
        }
    }

    fn expression(&mut self, expression: &IrExpression) {
        match expression {
            IrExpression::Integer(value, ty) => {
                self.word("Integer");
                self.int(*value);
                self.ty(*ty);
            }
            IrExpression::Float(value, ty) => {
                self.word("Float");
                self.uint(value.to_bits());
                self.ty(*ty);
            }
            IrExpression::String(value) => {
                self.word("String");
                self.string(value);
            }
            IrExpression::Character(value) => {
                self.word("Character");
                self.uint(u64::from(u32::from(*value)));
            }
            IrExpression::Boolean(value) => {
                self.word("Boolean");
                self.boolean(*value);
            }
            IrExpression::Local { slot, ty, span } => {
                self.word("Local");
                self.uint(*slot as u64);
                self.ty(*ty);
                self.span(*span);
            }
            IrExpression::Move { slot, ty } => {
                self.word("Move");
                self.uint(*slot as u64);
                self.ty(*ty);
            }
            IrExpression::StructValue { struct_id, fields } => {
                self.word("StructValue");
                self.uint(*struct_id as u64);
                self.list_len(fields.len());
                for (index, value) in fields {
                    self.uint(*index as u64);
                    self.expression(value);
                }
            }
            IrExpression::Field {
                value,
                struct_id,
                field_index,
                ty,
                span,
            } => {
                self.word("Field");
                self.expression(value);
                self.uint(*struct_id as u64);
                self.uint(*field_index as u64);
                self.ty(*ty);
                self.span(*span);
            }
            IrExpression::ReferenceField {
                pointer,
                struct_id,
                field_index,
                ty,
                span,
                borrowed,
            } => {
                self.word("ReferenceField");
                self.expression(pointer);
                self.uint(*struct_id as u64);
                self.uint(*field_index as u64);
                self.ty(*ty);
                self.span(*span);
                self.boolean(*borrowed);
            }
            IrExpression::ValueAddress {
                value,
                ty,
                pointer_type,
                span,
            } => {
                self.word("ValueAddress");
                self.expression(value);
                self.ty(*ty);
                self.ty(*pointer_type);
                self.span(*span);
            }
            IrExpression::Call {
                target,
                arguments,
                return_type,
            } => {
                self.word("Call");
                self.call_target(target);
                self.expressions(arguments);
                self.ty(*return_type);
            }
            IrExpression::If {
                condition,
                then_value,
                else_value,
                ty,
            } => {
                self.word("If");
                self.expression(condition);
                self.expression(then_value);
                self.expression(else_value);
                self.ty(*ty);
            }
            IrExpression::EnumMatch {
                value,
                enum_id,
                arms,
                ty,
            } => {
                self.word("EnumMatch");
                self.expression(value);
                self.uint(*enum_id as u64);
                self.list_len(arms.len());
                for arm in arms {
                    self.match_arm(arm);
                }
                self.ty(*ty);
            }
            IrExpression::Propagate {
                value,
                input_enum,
                output_enum,
                success_type,
                failure_type,
                success_variant,
                failure_variant,
                output_failure_variant,
                deferred,
            } => {
                self.word("Propagate");
                self.expression(value);
                self.uint(*input_enum as u64);
                self.uint(*output_enum as u64);
                self.ty(*success_type);
                self.optional_type(*failure_type);
                self.uint(*success_variant as u64);
                self.uint(*failure_variant as u64);
                self.uint(*output_failure_variant as u64);
                self.statements(deferred);
            }
            IrExpression::ArrayValue(elements) => {
                self.word("ArrayValue");
                self.expressions(elements);
            }
            IrExpression::ArrayRepeat { value, length } => {
                self.word("ArrayRepeat");
                self.expression(value);
                self.uint(*length as u64);
            }
            IrExpression::ArrayAsSlice {
                array,
                slice_id,
                length,
            } => {
                self.word("ArrayAsSlice");
                self.expression(array);
                self.uint(*slice_id as u64);
                self.uint(*length as u64);
            }
            IrExpression::ArrayIndex {
                array,
                index,
                ty,
                length,
            } => {
                self.word("ArrayIndex");
                self.expression(array);
                self.expression(index);
                self.ty(*ty);
                self.uint(*length as u64);
            }
            IrExpression::SliceIndex { slice, index, ty } => {
                self.word("SliceIndex");
                self.expression(slice);
                self.expression(index);
                self.ty(*ty);
            }
            IrExpression::SliceElementAddress {
                slice,
                index,
                ty,
                span,
            } => {
                self.word("SliceElementAddress");
                self.expression(slice);
                self.expression(index);
                self.ty(*ty);
                self.span(*span);
            }
            IrExpression::AddressOf {
                slot,
                ty,
                pointer_type,
                span,
            } => {
                self.word("AddressOf");
                self.uint(*slot as u64);
                self.ty(*ty);
                self.ty(*pointer_type);
                self.span(*span);
            }
            IrExpression::Dereference { pointer, ty } => {
                self.word("Dereference");
                self.expression(pointer);
                self.ty(*ty);
            }
            IrExpression::StringFindOption { value, enum_id } => {
                self.word("StringFindOption");
                self.expression(value);
                self.uint(*enum_id as u64);
            }
            IrExpression::StringAsStr(value) => {
                self.word("StringAsStr");
                self.expression(value);
            }
            IrExpression::Binary {
                op,
                left,
                right,
                ty,
            } => {
                self.word("Binary");
                self.word(binary_op_name(*op));
                self.expression(left);
                self.expression(right);
                self.ty(*ty);
            }
            IrExpression::Negate(value, ty) => {
                self.word("Negate");
                self.expression(value);
                self.ty(*ty);
            }
            IrExpression::Not(value) => {
                self.word("Not");
                self.expression(value);
            }
            IrExpression::BitNot(value, ty) => {
                self.word("BitNot");
                self.expression(value);
                self.ty(*ty);
            }
            IrExpression::FunctionAddress { function, ty } => {
                self.word("FunctionAddress");
                self.uint(*function as u64);
                self.ty(*ty);
            }
            IrExpression::Cast {
                value,
                source,
                target,
            } => {
                self.word("Cast");
                self.expression(value);
                self.ty(*source);
                self.ty(*target);
            }
        }
    }

    fn match_arm(&mut self, arm: &RynMatchArm) {
        self.optional_uint(arm.variant);
        self.list_len(arm.bindings.len());
        for binding in &arm.bindings {
            self.uint(binding.slot as u64);
            self.ty(binding.ty);
            self.uint(binding.field_index as u64);
        }
        self.expression(&arm.body);
    }
}

struct Decoder<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Decoder<'a> {
    fn skip_spaces(&mut self) {
        let bytes = self.text.as_bytes();
        while self.at < bytes.len() && bytes[self.at] == b' ' {
            self.at += 1;
        }
    }

    fn finish(&mut self) -> Result<(), String> {
        self.skip_spaces();
        if self.at == self.text.len() {
            Ok(())
        } else {
            Err(format!("trailing IR text at byte {}", self.at))
        }
    }

    fn token(&mut self) -> Result<&'a str, String> {
        self.skip_spaces();
        let start = self.at;
        let bytes = self.text.as_bytes();
        while self.at < bytes.len() && bytes[self.at] != b' ' {
            self.at += 1;
        }
        if start == self.at {
            return Err("unexpected end of IR text".into());
        }
        Ok(&self.text[start..self.at])
    }

    fn word(&mut self) -> Result<&'a str, String> {
        self.token()
    }

    fn expect_word(&mut self, expected: &str) -> Result<(), String> {
        let found = self.word()?;
        if found == expected {
            Ok(())
        } else {
            Err(format!("expected `{expected}` but found `{found}`"))
        }
    }

    fn uint(&mut self) -> Result<u64, String> {
        let token = self.token()?;
        token
            .parse()
            .map_err(|_| format!("expected an unsigned integer but found `{token}`"))
    }

    fn usize(&mut self) -> Result<usize, String> {
        Ok(self.uint()? as usize)
    }

    fn int(&mut self) -> Result<i128, String> {
        let token = self.token()?;
        token
            .parse()
            .map_err(|_| format!("expected an integer but found `{token}`"))
    }

    fn boolean(&mut self) -> Result<bool, String> {
        match self.uint()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(format!("expected a boolean but found {other}")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.skip_spaces();
        let bytes = self.text.as_bytes();
        let digits_start = self.at;
        while self.at < bytes.len() && bytes[self.at].is_ascii_digit() {
            self.at += 1;
        }
        if digits_start == self.at || self.at >= bytes.len() || bytes[self.at] != b':' {
            return Err("expected a length-prefixed string".into());
        }
        let length: usize = self.text[digits_start..self.at]
            .parse()
            .map_err(|_| "string length is out of range".to_string())?;
        self.at += 1;
        let end = self.at + length;
        let value = self
            .text
            .get(self.at..end)
            .ok_or("string extends past the end of the IR text")?
            .to_owned();
        self.at = end;
        Ok(value)
    }

    fn span(&mut self) -> Result<Span, String> {
        Ok(Span {
            start: self.usize()?,
            end: self.usize()?,
        })
    }

    fn optional_uint(&mut self) -> Result<Option<usize>, String> {
        match self.word()? {
            "-" => Ok(None),
            "+" => Ok(Some(self.usize()?)),
            other => Err(format!("expected an optional value but found `{other}`")),
        }
    }

    fn optional_type(&mut self) -> Result<Option<Type>, String> {
        match self.word()? {
            "-" => Ok(None),
            "+" => Ok(Some(self.ty()?)),
            other => Err(format!("expected an optional type but found `{other}`")),
        }
    }

    fn list_len(&mut self) -> Result<usize, String> {
        self.usize()
    }

    fn ir(&mut self) -> Result<RynIr, String> {
        let main_index = self.usize()?;
        let instant_drop = self.optional_uint()?;
        let structs = (0..self.list_len()?)
            .map(|_| self.struct_definition())
            .collect::<Result<Vec<_>, _>>()?;
        let enums = (0..self.list_len()?)
            .map(|_| self.enum_definition())
            .collect::<Result<Vec<_>, _>>()?;
        let functions = (0..self.list_len()?)
            .map(|_| self.function())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RynIr {
            functions,
            main_index,
            structs,
            enums,
            instant_drop,
        })
    }

    fn struct_definition(&mut self) -> Result<RynStruct, String> {
        let name = self.string()?;
        let field_count = self.list_len()?;
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            fields.push(RynStructField {
                name: self.string()?,
                ty: self.ty()?,
                slot_offset: self.usize()?,
                public: self.boolean()?,
            });
        }
        Ok(RynStruct {
            name,
            fields,
            slot_count: self.usize()?,
            repr_c: self.boolean()?,
            drop_function: self.optional_uint()?,
            derives_clone: self.boolean()?,
            derives_hash: self.boolean()?,
            module_path: self.string()?,
        })
    }

    fn enum_definition(&mut self) -> Result<RynEnum, String> {
        let name = self.string()?;
        let variant_count = self.list_len()?;
        let mut variants = Vec::with_capacity(variant_count);
        for _ in 0..variant_count {
            let name = self.string()?;
            let field_count = self.list_len()?;
            let mut fields = Vec::with_capacity(field_count);
            for _ in 0..field_count {
                fields.push(self.ty()?);
            }
            variants.push(RynEnumVariant { name, fields });
        }
        Ok(RynEnum { name, variants })
    }

    fn function(&mut self) -> Result<RynFunction, String> {
        let parameter_count = self.list_len()?;
        let mut parameters = Vec::with_capacity(parameter_count);
        for _ in 0..parameter_count {
            parameters.push(LocalBinding {
                slot: self.usize()?,
                ty: self.ty()?,
            });
        }
        let external_symbol = match self.word()? {
            "-" => None,
            "+" => Some(self.string()?),
            other => return Err(format!("expected an optional symbol but found `{other}`")),
        };
        let return_type = self.optional_type()?;
        let reference_return_parameter = self.optional_uint()?;
        let return_value = match self.word()? {
            "-" => None,
            "+" => Some(self.expression()?),
            other => return Err(format!("expected an optional value but found `{other}`")),
        };
        let statements = self.statements()?;
        let local_count = self.list_len()?;
        let mut local_types = Vec::with_capacity(local_count);
        for _ in 0..local_count {
            local_types.push(self.local_type()?);
        }
        let owned_count = self.list_len()?;
        let mut owned_slot_types = Vec::with_capacity(owned_count);
        for _ in 0..owned_count {
            owned_slot_types.push(self.optional_type()?);
        }
        let addressed_count = self.list_len()?;
        let mut addressed_slot_types = Vec::with_capacity(addressed_count);
        for _ in 0..addressed_count {
            addressed_slot_types.push(self.optional_type()?);
        }
        Ok(RynFunction {
            parameters,
            external_symbol,
            return_type,
            reference_return_parameter,
            return_value,
            statements,
            local_types,
            owned_slot_types,
            addressed_slot_types,
        })
    }

    fn local_type(&mut self) -> Result<LocalType, String> {
        Ok(match self.word()? {
            "I8" => LocalType::I8,
            "I16" => LocalType::I16,
            "I32" => LocalType::I32,
            "I64" => LocalType::I64,
            "F32" => LocalType::F32,
            "F64" => LocalType::F64,
            "Ptr" => LocalType::Ptr,
            "OwnedPtr" => LocalType::OwnedPtr,
            other => return Err(format!("unknown local type `{other}`")),
        })
    }

    fn ty(&mut self) -> Result<Type, String> {
        Ok(match self.word()? {
            "I8" => Type::I8,
            "I16" => Type::I16,
            "I32" => Type::I32,
            "I64" => Type::I64,
            "U8" => Type::U8,
            "U16" => Type::U16,
            "U32" => Type::U32,
            "U64" => Type::U64,
            "F32" => Type::F32,
            "F64" => Type::F64,
            "Str" => Type::Str,
            "OwnedString" => Type::OwnedString,
            "Char" => Type::Char,
            "Bool" => Type::Bool,
            "Struct" => Type::Struct(self.usize()?),
            "Enum" => Type::Enum(self.usize()?),
            "Vec" => Type::Vec(intern_vec_elem(self.ty()?)),
            "Slice" => Type::Slice(intern_vec_elem(self.ty()?)),
            "Array" => {
                let element = self.ty()?;
                let length = self.usize()?;
                Type::Array(intern_array(element, length))
            }
            "Map" => {
                let key = self.ty()?;
                let value = self.ty()?;
                Type::Map(intern_map(key, value))
            }
            "Set" => {
                let key = self.ty()?;
                let value = self.ty()?;
                Type::Set(intern_map(key, value))
            }
            "Reference" => {
                let target = self.ty()?;
                let mutable = self.boolean()?;
                Type::Reference(intern_pointer_target(target), mutable)
            }
            "RawPointer" => Type::RawPointer(intern_pointer_target(self.ty()?)),
            "FunctionPointer" => {
                let parameter_count = self.list_len()?;
                let mut parameters = Vec::with_capacity(parameter_count);
                for _ in 0..parameter_count {
                    parameters.push(self.ty()?);
                }
                let result = self.optional_type()?;
                let extern_c = self.boolean()?;
                Type::FunctionPointer(intern_function_pointer(FunctionPointerSignature {
                    parameters,
                    result,
                    extern_c,
                }))
            }
            other => return Err(format!("unknown type `{other}`")),
        })
    }

    fn statements(&mut self) -> Result<Vec<IrStatement>, String> {
        let count = self.list_len()?;
        let mut statements = Vec::with_capacity(count);
        for _ in 0..count {
            statements.push(self.statement()?);
        }
        Ok(statements)
    }

    fn expressions(&mut self) -> Result<Vec<IrExpression>, String> {
        let count = self.list_len()?;
        let mut expressions = Vec::with_capacity(count);
        for _ in 0..count {
            expressions.push(self.expression()?);
        }
        Ok(expressions)
    }

    fn statement(&mut self) -> Result<IrStatement, String> {
        Ok(match self.word()? {
            "Block" => IrStatement::Block(self.statements()?),
            "Drop" => {
                let count = self.list_len()?;
                let mut slots = Vec::with_capacity(count);
                for _ in 0..count {
                    slots.push((self.usize()?, self.ty()?));
                }
                IrStatement::Drop { slots }
            }
            "Let" => IrStatement::Let {
                slot: self.usize()?,
                ty: self.ty()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "Assign" => IrStatement::Assign {
                slot: self.usize()?,
                ty: self.ty()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "ArrayAssign" => IrStatement::ArrayAssign {
                slot: self.usize()?,
                element: self.ty()?,
                length: self.usize()?,
                index: self.expression()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "DereferenceAssign" => IrStatement::DereferenceAssign {
                pointer: self.expression()?,
                ty: self.ty()?,
                value: self.expression()?,
            },
            "FieldAssign" => IrStatement::FieldAssign {
                slot: self.usize()?,
                ty: self.ty()?,
                value: self.expression()?,
            },
            "ReferenceFieldAssign" => IrStatement::ReferenceFieldAssign {
                pointer: self.expression()?,
                struct_id: self.usize()?,
                field_index: self.usize()?,
                ty: self.ty()?,
                value: self.expression()?,
            },
            "Print" => IrStatement::Print {
                value: self.expression()?,
                ty: self.ty()?,
            },
            "PrintTemplate" => {
                let count = self.list_len()?;
                let mut parts = Vec::with_capacity(count);
                for _ in 0..count {
                    parts.push(self.print_part()?);
                }
                IrStatement::PrintTemplate(parts)
            }
            "CallStatement" => IrStatement::Call {
                target: self.call_target()?,
                arguments: self.expressions()?,
            },
            "IfStatement" => IrStatement::If {
                condition: self.expression()?,
                then_body: self.statements()?,
                else_body: self.statements()?,
            },
            "While" => IrStatement::While {
                setup: self.statements()?,
                condition: self.expression()?,
                body: self.statements()?,
            },
            "For" => IrStatement::For {
                slot: self.usize()?,
                end_slot: self.usize()?,
                ty: self.ty()?,
                start: self.expression()?,
                end: self.expression()?,
                inclusive: self.boolean()?,
                body: self.statements()?,
            },
            "Break" => IrStatement::Break,
            "Continue" => IrStatement::Continue,
            "Return" => IrStatement::Return {
                value: match self.word()? {
                    "-" => None,
                    "+" => Some(self.expression()?),
                    other => return Err(format!("expected an optional value but found `{other}`")),
                },
            },
            other => return Err(format!("unknown statement `{other}`")),
        })
    }

    fn print_part(&mut self) -> Result<IrPrintPart, String> {
        Ok(match self.word()? {
            "Text" => IrPrintPart::Text(self.string()?),
            "Value" => IrPrintPart::Value {
                value: self.expression()?,
                ty: self.ty()?,
            },
            other => return Err(format!("unknown print part `{other}`")),
        })
    }

    fn call_target(&mut self) -> Result<IrCallTarget, String> {
        Ok(match self.word()? {
            "Function" => IrCallTarget::Function(self.usize()?),
            "ArgumentCount" => IrCallTarget::ArgumentCount,
            "Argument" => IrCallTarget::Argument,
            "String" => IrCallTarget::String(op_by_name(STRING_OP_NAMES, self.word()?)?),
            "Filesystem" => {
                IrCallTarget::Filesystem(op_by_name(FILESYSTEM_OP_NAMES, self.word()?)?)
            }
            "TryReadFile" => IrCallTarget::TryReadFile(self.usize()?),
            "ReadFileResult" => IrCallTarget::ReadFileResult(self.usize()?),
            "System" => IrCallTarget::System(op_by_name(SYSTEM_OP_NAMES, self.word()?)?),
            "IndirectFunctionPointer" => IrCallTarget::IndirectFunctionPointer(self.usize()?),
            "Vec" => IrCallTarget::Vec(op_by_name(VEC_OP_NAMES, self.word()?)?, self.usize()?),
            "VecSlice" => IrCallTarget::VecSlice(self.usize()?),
            "SliceLen" => IrCallTarget::SliceLen,
            "Map" => IrCallTarget::Map(op_by_name(MAP_OP_NAMES, self.word()?)?, self.usize()?),
            "Set" => IrCallTarget::Set(op_by_name(MAP_OP_NAMES, self.word()?)?, self.usize()?),
            "EnumNew" => IrCallTarget::EnumNew {
                enum_id: self.usize()?,
                tag: self.usize()?,
            },
            "EnumPredicate" => IrCallTarget::EnumPredicate(
                op_by_name(ENUM_PREDICATE_NAMES, self.word()?)?,
                self.usize()?,
            ),
            "EnumUnwrapOr" => IrCallTarget::EnumUnwrapOr {
                enum_id: self.usize()?,
                value_type: self.ty()?,
            },
            "EnumUnwrap" => IrCallTarget::EnumUnwrap {
                enum_id: self.usize()?,
                value_type: self.ty()?,
                message: self.boolean()?,
                success_tag: self.usize()?,
                failure: op_by_name(SYSTEM_OP_NAMES, self.word()?)?,
            },
            "VecGetOption" => IrCallTarget::VecGetOption {
                elem_id: self.usize()?,
                option_id: self.usize()?,
                pop: self.boolean()?,
            },
            other => return Err(format!("unknown call target `{other}`")),
        })
    }

    fn expression(&mut self) -> Result<IrExpression, String> {
        Ok(match self.word()? {
            "Integer" => IrExpression::Integer(self.int()?, self.ty()?),
            "Float" => IrExpression::Float(f64::from_bits(self.uint()?), self.ty()?),
            "String" => IrExpression::String(self.string()?),
            "Character" => {
                let scalar = self.uint()?;
                let value = u32::try_from(scalar)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("invalid character scalar {scalar}"))?;
                IrExpression::Character(value)
            }
            "Boolean" => IrExpression::Boolean(self.boolean()?),
            "Local" => IrExpression::Local {
                slot: self.usize()?,
                ty: self.ty()?,
                span: self.span()?,
            },
            "Move" => IrExpression::Move {
                slot: self.usize()?,
                ty: self.ty()?,
            },
            "StructValue" => {
                let struct_id = self.usize()?;
                let count = self.list_len()?;
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    fields.push((self.usize()?, self.expression()?));
                }
                IrExpression::StructValue { struct_id, fields }
            }
            "Field" => IrExpression::Field {
                value: Box::new(self.expression()?),
                struct_id: self.usize()?,
                field_index: self.usize()?,
                ty: self.ty()?,
                span: self.span()?,
            },
            "ReferenceField" => IrExpression::ReferenceField {
                pointer: Box::new(self.expression()?),
                struct_id: self.usize()?,
                field_index: self.usize()?,
                ty: self.ty()?,
                span: self.span()?,
                borrowed: self.boolean()?,
            },
            "ValueAddress" => IrExpression::ValueAddress {
                value: Box::new(self.expression()?),
                ty: self.ty()?,
                pointer_type: self.ty()?,
                span: self.span()?,
            },
            "Call" => IrExpression::Call {
                target: self.call_target()?,
                arguments: self.expressions()?,
                return_type: self.ty()?,
            },
            "If" => IrExpression::If {
                condition: Box::new(self.expression()?),
                then_value: Box::new(self.expression()?),
                else_value: Box::new(self.expression()?),
                ty: self.ty()?,
            },
            "EnumMatch" => {
                let value = Box::new(self.expression()?);
                let enum_id = self.usize()?;
                let count = self.list_len()?;
                let mut arms = Vec::with_capacity(count);
                for _ in 0..count {
                    arms.push(self.match_arm()?);
                }
                IrExpression::EnumMatch {
                    value,
                    enum_id,
                    arms,
                    ty: self.ty()?,
                }
            }
            "Propagate" => IrExpression::Propagate {
                value: Box::new(self.expression()?),
                input_enum: self.usize()?,
                output_enum: self.usize()?,
                success_type: self.ty()?,
                failure_type: self.optional_type()?,
                success_variant: self.usize()?,
                failure_variant: self.usize()?,
                output_failure_variant: self.usize()?,
                deferred: self.statements()?,
            },
            "ArrayValue" => IrExpression::ArrayValue(self.expressions()?),
            "ArrayRepeat" => IrExpression::ArrayRepeat {
                value: Box::new(self.expression()?),
                length: self.usize()?,
            },
            "ArrayAsSlice" => IrExpression::ArrayAsSlice {
                array: Box::new(self.expression()?),
                slice_id: self.usize()?,
                length: self.usize()?,
            },
            "ArrayIndex" => IrExpression::ArrayIndex {
                array: Box::new(self.expression()?),
                index: Box::new(self.expression()?),
                ty: self.ty()?,
                length: self.usize()?,
            },
            "SliceIndex" => IrExpression::SliceIndex {
                slice: Box::new(self.expression()?),
                index: Box::new(self.expression()?),
                ty: self.ty()?,
            },
            "SliceElementAddress" => IrExpression::SliceElementAddress {
                slice: Box::new(self.expression()?),
                index: Box::new(self.expression()?),
                ty: self.ty()?,
                span: self.span()?,
            },
            "AddressOf" => IrExpression::AddressOf {
                slot: self.usize()?,
                ty: self.ty()?,
                pointer_type: self.ty()?,
                span: self.span()?,
            },
            "Dereference" => IrExpression::Dereference {
                pointer: Box::new(self.expression()?),
                ty: self.ty()?,
            },
            "StringFindOption" => IrExpression::StringFindOption {
                value: Box::new(self.expression()?),
                enum_id: self.usize()?,
            },
            "StringAsStr" => IrExpression::StringAsStr(Box::new(self.expression()?)),
            "Binary" => IrExpression::Binary {
                op: op_by_name(BINARY_OP_NAMES, self.word()?)?,
                left: Box::new(self.expression()?),
                right: Box::new(self.expression()?),
                ty: self.ty()?,
            },
            "Negate" => IrExpression::Negate(Box::new(self.expression()?), self.ty()?),
            "Not" => IrExpression::Not(Box::new(self.expression()?)),
            "BitNot" => IrExpression::BitNot(Box::new(self.expression()?), self.ty()?),
            "FunctionAddress" => IrExpression::FunctionAddress {
                function: self.usize()?,
                ty: self.ty()?,
            },
            "Cast" => IrExpression::Cast {
                value: Box::new(self.expression()?),
                source: self.ty()?,
                target: self.ty()?,
            },
            other => return Err(format!("unknown expression `{other}`")),
        })
    }

    fn match_arm(&mut self) -> Result<RynMatchArm, String> {
        let variant = self.optional_uint()?;
        let count = self.list_len()?;
        let mut bindings = Vec::with_capacity(count);
        for _ in 0..count {
            bindings.push(RynArmBinding {
                slot: self.usize()?,
                ty: self.ty()?,
                field_index: self.usize()?,
            });
        }
        Ok(RynMatchArm {
            variant,
            bindings,
            body: self.expression()?,
        })
    }
}

fn op_by_name<T: Copy>(table: &[(&str, T)], name: &str) -> Result<T, String> {
    table
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, value)| *value)
        .ok_or_else(|| format!("unknown operation `{name}`"))
}

// Exhaustive names of the operation enums, generated from their definitions.
fn string_op_name(value: StringOp) -> &'static str {
    match value {
        StringOp::New => "New",
        StringOp::Clone => "Clone",
        StringOp::Drop => "Drop",
        StringOp::Print => "Print",
        StringOp::Equals => "Equals",
        StringOp::Len => "Len",
        StringOp::CharCount => "CharCount",
        StringOp::ByteAt => "ByteAt",
        StringOp::CharAt => "CharAt",
        StringOp::StrCharCount => "StrCharCount",
        StringOp::StrCharAt => "StrCharAt",
        StringOp::Slice => "Slice",
        StringOp::SliceChars => "SliceChars",
        StringOp::Data => "Data",
        StringOp::Trim => "Trim",
        StringOp::Clear => "Clear",
        StringOp::Push => "Push",
        StringOp::AppendStr => "AppendStr",
        StringOp::AppendString => "AppendString",
        StringOp::ConcatStr => "ConcatStr",
        StringOp::ConcatString => "ConcatString",
        StringOp::StartsWithStr => "StartsWithStr",
        StringOp::StartsWithString => "StartsWithString",
        StringOp::EndsWithStr => "EndsWithStr",
        StringOp::EndsWithString => "EndsWithString",
        StringOp::ContainsStr => "ContainsStr",
        StringOp::ContainsString => "ContainsString",
        StringOp::FindStr => "FindStr",
        StringOp::FindString => "FindString",
        StringOp::SplitStr => "SplitStr",
        StringOp::SplitString => "SplitString",
        StringOp::PrintChar => "PrintChar",
        StringOp::FromI8 => "FromI8",
        StringOp::FromI16 => "FromI16",
        StringOp::FromI32 => "FromI32",
        StringOp::FromI64 => "FromI64",
        StringOp::FromU8 => "FromU8",
        StringOp::FromU16 => "FromU16",
        StringOp::FromU32 => "FromU32",
        StringOp::FromU64 => "FromU64",
        StringOp::FromF32 => "FromF32",
        StringOp::FromF64 => "FromF64",
        StringOp::ToI8 => "ToI8",
        StringOp::ToI16 => "ToI16",
        StringOp::ToI32 => "ToI32",
        StringOp::ToI64 => "ToI64",
        StringOp::ToU8 => "ToU8",
        StringOp::ToU16 => "ToU16",
        StringOp::ToU32 => "ToU32",
        StringOp::ToU64 => "ToU64",
        StringOp::ToF32 => "ToF32",
        StringOp::ToF64 => "ToF64",
        StringOp::TryParse => "TryParse",
        StringOp::CharFromU32 => "CharFromU32",
        StringOp::IsEmpty => "IsEmpty",
        StringOp::TrimStart => "TrimStart",
        StringOp::TrimEnd => "TrimEnd",
        StringOp::ToLower => "ToLower",
        StringOp::ToUpper => "ToUpper",
        StringOp::ReplaceStr => "ReplaceStr",
        StringOp::Lines => "Lines",
        StringOp::Chars => "Chars",
        StringOp::Bytes => "Bytes",
        StringOp::Repeat => "Repeat",
        StringOp::Reverse => "Reverse",
    }
}

const STRING_OP_NAMES: &[(&str, StringOp)] = &[
    ("New", StringOp::New),
    ("Clone", StringOp::Clone),
    ("Drop", StringOp::Drop),
    ("Print", StringOp::Print),
    ("Equals", StringOp::Equals),
    ("Len", StringOp::Len),
    ("CharCount", StringOp::CharCount),
    ("ByteAt", StringOp::ByteAt),
    ("CharAt", StringOp::CharAt),
    ("StrCharCount", StringOp::StrCharCount),
    ("StrCharAt", StringOp::StrCharAt),
    ("Slice", StringOp::Slice),
    ("SliceChars", StringOp::SliceChars),
    ("Data", StringOp::Data),
    ("Trim", StringOp::Trim),
    ("Clear", StringOp::Clear),
    ("Push", StringOp::Push),
    ("AppendStr", StringOp::AppendStr),
    ("AppendString", StringOp::AppendString),
    ("ConcatStr", StringOp::ConcatStr),
    ("ConcatString", StringOp::ConcatString),
    ("StartsWithStr", StringOp::StartsWithStr),
    ("StartsWithString", StringOp::StartsWithString),
    ("EndsWithStr", StringOp::EndsWithStr),
    ("EndsWithString", StringOp::EndsWithString),
    ("ContainsStr", StringOp::ContainsStr),
    ("ContainsString", StringOp::ContainsString),
    ("FindStr", StringOp::FindStr),
    ("FindString", StringOp::FindString),
    ("SplitStr", StringOp::SplitStr),
    ("SplitString", StringOp::SplitString),
    ("PrintChar", StringOp::PrintChar),
    ("FromI8", StringOp::FromI8),
    ("FromI16", StringOp::FromI16),
    ("FromI32", StringOp::FromI32),
    ("FromI64", StringOp::FromI64),
    ("FromU8", StringOp::FromU8),
    ("FromU16", StringOp::FromU16),
    ("FromU32", StringOp::FromU32),
    ("FromU64", StringOp::FromU64),
    ("FromF32", StringOp::FromF32),
    ("FromF64", StringOp::FromF64),
    ("ToI8", StringOp::ToI8),
    ("ToI16", StringOp::ToI16),
    ("ToI32", StringOp::ToI32),
    ("ToI64", StringOp::ToI64),
    ("ToU8", StringOp::ToU8),
    ("ToU16", StringOp::ToU16),
    ("ToU32", StringOp::ToU32),
    ("ToU64", StringOp::ToU64),
    ("ToF32", StringOp::ToF32),
    ("ToF64", StringOp::ToF64),
    ("TryParse", StringOp::TryParse),
    ("CharFromU32", StringOp::CharFromU32),
    ("IsEmpty", StringOp::IsEmpty),
    ("TrimStart", StringOp::TrimStart),
    ("TrimEnd", StringOp::TrimEnd),
    ("ToLower", StringOp::ToLower),
    ("ToUpper", StringOp::ToUpper),
    ("ReplaceStr", StringOp::ReplaceStr),
    ("Lines", StringOp::Lines),
    ("Chars", StringOp::Chars),
    ("Bytes", StringOp::Bytes),
    ("Repeat", StringOp::Repeat),
    ("Reverse", StringOp::Reverse),
];

fn filesystem_op_name(value: FilesystemOp) -> &'static str {
    match value {
        FilesystemOp::ReadFile => "ReadFile",
        FilesystemOp::WriteFile => "WriteFile",
        FilesystemOp::CreateDir => "CreateDir",
        FilesystemOp::CreateDirAll => "CreateDirAll",
        FilesystemOp::DeleteFile => "DeleteFile",
        FilesystemOp::DeleteDir => "DeleteDir",
        FilesystemOp::Exists => "Exists",
        FilesystemOp::IsFile => "IsFile",
        FilesystemOp::IsDirectory => "IsDirectory",
        FilesystemOp::DeleteDirAll => "DeleteDirAll",
        FilesystemOp::ReadDir => "ReadDir",
        FilesystemOp::PathJoin => "PathJoin",
        FilesystemOp::PathParent => "PathParent",
        FilesystemOp::PathFileName => "PathFileName",
        FilesystemOp::PathExtension => "PathExtension",
        FilesystemOp::PathIsAbsolute => "PathIsAbsolute",
        FilesystemOp::CopyFile => "CopyFile",
        FilesystemOp::Rename => "Rename",
        FilesystemOp::PathAbsolute => "PathAbsolute",
        FilesystemOp::PathCanonical => "PathCanonical",
        FilesystemOp::FileOpen => "FileOpen",
        FilesystemOp::FileCreate => "FileCreate",
        FilesystemOp::FileRead => "FileRead",
        FilesystemOp::FileReadLine => "FileReadLine",
        FilesystemOp::FileWrite => "FileWrite",
        FilesystemOp::FileFlush => "FileFlush",
        FilesystemOp::FileClose => "FileClose",
        FilesystemOp::FileDrop => "FileDrop",
        FilesystemOp::AppendFile => "AppendFile",
    }
}

const FILESYSTEM_OP_NAMES: &[(&str, FilesystemOp)] = &[
    ("ReadFile", FilesystemOp::ReadFile),
    ("WriteFile", FilesystemOp::WriteFile),
    ("CreateDir", FilesystemOp::CreateDir),
    ("CreateDirAll", FilesystemOp::CreateDirAll),
    ("DeleteFile", FilesystemOp::DeleteFile),
    ("DeleteDir", FilesystemOp::DeleteDir),
    ("Exists", FilesystemOp::Exists),
    ("IsFile", FilesystemOp::IsFile),
    ("IsDirectory", FilesystemOp::IsDirectory),
    ("DeleteDirAll", FilesystemOp::DeleteDirAll),
    ("ReadDir", FilesystemOp::ReadDir),
    ("PathJoin", FilesystemOp::PathJoin),
    ("PathParent", FilesystemOp::PathParent),
    ("PathFileName", FilesystemOp::PathFileName),
    ("PathExtension", FilesystemOp::PathExtension),
    ("PathIsAbsolute", FilesystemOp::PathIsAbsolute),
    ("CopyFile", FilesystemOp::CopyFile),
    ("Rename", FilesystemOp::Rename),
    ("PathAbsolute", FilesystemOp::PathAbsolute),
    ("PathCanonical", FilesystemOp::PathCanonical),
    ("FileOpen", FilesystemOp::FileOpen),
    ("FileCreate", FilesystemOp::FileCreate),
    ("FileRead", FilesystemOp::FileRead),
    ("FileReadLine", FilesystemOp::FileReadLine),
    ("FileWrite", FilesystemOp::FileWrite),
    ("FileFlush", FilesystemOp::FileFlush),
    ("FileClose", FilesystemOp::FileClose),
    ("FileDrop", FilesystemOp::FileDrop),
    ("AppendFile", FilesystemOp::AppendFile),
];

fn system_op_name(value: SystemOp) -> &'static str {
    match value {
        SystemOp::EnvExists => "EnvExists",
        SystemOp::EnvOr => "EnvOr",
        SystemOp::SetEnv => "SetEnv",
        SystemOp::RemoveEnv => "RemoveEnv",
        SystemOp::ReadStdin => "ReadStdin",
        SystemOp::ReadStdinLine => "ReadStdinLine",
        SystemOp::Ask => "Ask",
        SystemOp::WriteStdout => "WriteStdout",
        SystemOp::FlushStdout => "FlushStdout",
        SystemOp::WriteStderr => "WriteStderr",
        SystemOp::FlushStderr => "FlushStderr",
        SystemOp::RunProcess => "RunProcess",
        SystemOp::RunProcessArgs => "RunProcessArgs",
        SystemOp::ProcessNew => "ProcessNew",
        SystemOp::ProcessArg => "ProcessArg",
        SystemOp::ProcessArgs => "ProcessArgs",
        SystemOp::ProcessEnv => "ProcessEnv",
        SystemOp::ProcessCwd => "ProcessCwd",
        SystemOp::ProcessSpawn => "ProcessSpawn",
        SystemOp::ProcessWait => "ProcessWait",
        SystemOp::ProcessKill => "ProcessKill",
        SystemOp::ProcessDrop => "ProcessDrop",
        SystemOp::Panic => "Panic",
        SystemOp::Exit => "Exit",
        SystemOp::Assert => "Assert",
        SystemOp::AssertMessage => "AssertMessage",
        SystemOp::LoadLibrary => "LoadLibrary",
        SystemOp::LoadSymbol => "LoadSymbol",
        SystemOp::UnloadLibrary => "UnloadLibrary",
        SystemOp::PointerIsNull => "PointerIsNull",
        SystemOp::CallCI32One => "CallCI32One",
        SystemOp::MathAbs => "MathAbs",
        SystemOp::MathMin => "MathMin",
        SystemOp::MathMax => "MathMax",
        SystemOp::MathClamp => "MathClamp",
        SystemOp::MathSqrt => "MathSqrt",
        SystemOp::MathPow => "MathPow",
        SystemOp::MathFloor => "MathFloor",
        SystemOp::MathCeil => "MathCeil",
        SystemOp::MathRound => "MathRound",
        SystemOp::MathSin => "MathSin",
        SystemOp::MathCos => "MathCos",
        SystemOp::MathTan => "MathTan",
        SystemOp::MathLog => "MathLog",
        SystemOp::MathLog2 => "MathLog2",
        SystemOp::MathLog10 => "MathLog10",
        SystemOp::Random => "Random",
        SystemOp::RandomRange => "RandomRange",
        SystemOp::Sleep => "Sleep",
        SystemOp::YieldThread => "YieldThread",
        SystemOp::EnvGet => "EnvGet",
        SystemOp::CurrentDir => "CurrentDir",
        SystemOp::SetCurrentDir => "SetCurrentDir",
        SystemOp::HomeDir => "HomeDir",
        SystemOp::TempDir => "TempDir",
        SystemOp::ExecutablePath => "ExecutablePath",
        SystemOp::Os => "Os",
        SystemOp::Arch => "Arch",
        SystemOp::CpuCount => "CpuCount",
        SystemOp::Hostname => "Hostname",
        SystemOp::TimeUnix => "TimeUnix",
        SystemOp::TimeMonotonic => "TimeMonotonic",
        SystemOp::TimeElapsed => "TimeElapsed",
        SystemOp::OptionUnwrapFailed => "OptionUnwrapFailed",
        SystemOp::OptionExpectFailed => "OptionExpectFailed",
        SystemOp::ResultUnwrapFailed => "ResultUnwrapFailed",
        SystemOp::ProcessCapture => "ProcessCapture",
        SystemOp::ProcessCaptureExitCode => "ProcessCaptureExitCode",
        SystemOp::ProcessCaptureStdout => "ProcessCaptureStdout",
        SystemOp::ProcessCaptureStderr => "ProcessCaptureStderr",
        SystemOp::ProcessCaptureDrop => "ProcessCaptureDrop",
        SystemOp::TcpConnect => "TcpConnect",
        SystemOp::TcpRead => "TcpRead",
        SystemOp::TcpWrite => "TcpWrite",
        SystemOp::TcpClose => "TcpClose",
        SystemOp::TcpDrop => "TcpDrop",
        SystemOp::TcpListenerBind => "TcpListenerBind",
        SystemOp::TcpAccept => "TcpAccept",
        SystemOp::TcpListenerClose => "TcpListenerClose",
        SystemOp::TcpListenerDrop => "TcpListenerDrop",
        SystemOp::DnsResolve => "DnsResolve",
        SystemOp::UdpBind => "UdpBind",
        SystemOp::UdpConnect => "UdpConnect",
        SystemOp::UdpRead => "UdpRead",
        SystemOp::UdpWrite => "UdpWrite",
        SystemOp::UdpClose => "UdpClose",
        SystemOp::UdpDrop => "UdpDrop",
        SystemOp::ThreadSpawn => "ThreadSpawn",
        SystemOp::ThreadJoin => "ThreadJoin",
        SystemOp::ThreadId => "ThreadId",
        SystemOp::ThreadDrop => "ThreadDrop",
        SystemOp::KeyDown => "KeyDown",
        SystemOp::KeyPressed => "KeyPressed",
        SystemOp::KeyReleased => "KeyReleased",
        SystemOp::ReadKey => "ReadKey",
    }
}

const SYSTEM_OP_NAMES: &[(&str, SystemOp)] = &[
    ("EnvExists", SystemOp::EnvExists),
    ("EnvOr", SystemOp::EnvOr),
    ("SetEnv", SystemOp::SetEnv),
    ("RemoveEnv", SystemOp::RemoveEnv),
    ("ReadStdin", SystemOp::ReadStdin),
    ("ReadStdinLine", SystemOp::ReadStdinLine),
    ("Ask", SystemOp::Ask),
    ("WriteStdout", SystemOp::WriteStdout),
    ("FlushStdout", SystemOp::FlushStdout),
    ("WriteStderr", SystemOp::WriteStderr),
    ("FlushStderr", SystemOp::FlushStderr),
    ("RunProcess", SystemOp::RunProcess),
    ("RunProcessArgs", SystemOp::RunProcessArgs),
    ("ProcessNew", SystemOp::ProcessNew),
    ("ProcessArg", SystemOp::ProcessArg),
    ("ProcessArgs", SystemOp::ProcessArgs),
    ("ProcessEnv", SystemOp::ProcessEnv),
    ("ProcessCwd", SystemOp::ProcessCwd),
    ("ProcessSpawn", SystemOp::ProcessSpawn),
    ("ProcessWait", SystemOp::ProcessWait),
    ("ProcessKill", SystemOp::ProcessKill),
    ("ProcessDrop", SystemOp::ProcessDrop),
    ("Panic", SystemOp::Panic),
    ("Exit", SystemOp::Exit),
    ("Assert", SystemOp::Assert),
    ("AssertMessage", SystemOp::AssertMessage),
    ("LoadLibrary", SystemOp::LoadLibrary),
    ("LoadSymbol", SystemOp::LoadSymbol),
    ("UnloadLibrary", SystemOp::UnloadLibrary),
    ("PointerIsNull", SystemOp::PointerIsNull),
    ("CallCI32One", SystemOp::CallCI32One),
    ("MathAbs", SystemOp::MathAbs),
    ("MathMin", SystemOp::MathMin),
    ("MathMax", SystemOp::MathMax),
    ("MathClamp", SystemOp::MathClamp),
    ("MathSqrt", SystemOp::MathSqrt),
    ("MathPow", SystemOp::MathPow),
    ("MathFloor", SystemOp::MathFloor),
    ("MathCeil", SystemOp::MathCeil),
    ("MathRound", SystemOp::MathRound),
    ("MathSin", SystemOp::MathSin),
    ("MathCos", SystemOp::MathCos),
    ("MathTan", SystemOp::MathTan),
    ("MathLog", SystemOp::MathLog),
    ("MathLog2", SystemOp::MathLog2),
    ("MathLog10", SystemOp::MathLog10),
    ("Random", SystemOp::Random),
    ("RandomRange", SystemOp::RandomRange),
    ("Sleep", SystemOp::Sleep),
    ("YieldThread", SystemOp::YieldThread),
    ("EnvGet", SystemOp::EnvGet),
    ("CurrentDir", SystemOp::CurrentDir),
    ("SetCurrentDir", SystemOp::SetCurrentDir),
    ("HomeDir", SystemOp::HomeDir),
    ("TempDir", SystemOp::TempDir),
    ("ExecutablePath", SystemOp::ExecutablePath),
    ("Os", SystemOp::Os),
    ("Arch", SystemOp::Arch),
    ("CpuCount", SystemOp::CpuCount),
    ("Hostname", SystemOp::Hostname),
    ("TimeUnix", SystemOp::TimeUnix),
    ("TimeMonotonic", SystemOp::TimeMonotonic),
    ("TimeElapsed", SystemOp::TimeElapsed),
    ("OptionUnwrapFailed", SystemOp::OptionUnwrapFailed),
    ("OptionExpectFailed", SystemOp::OptionExpectFailed),
    ("ResultUnwrapFailed", SystemOp::ResultUnwrapFailed),
    ("ProcessCapture", SystemOp::ProcessCapture),
    ("ProcessCaptureExitCode", SystemOp::ProcessCaptureExitCode),
    ("ProcessCaptureStdout", SystemOp::ProcessCaptureStdout),
    ("ProcessCaptureStderr", SystemOp::ProcessCaptureStderr),
    ("ProcessCaptureDrop", SystemOp::ProcessCaptureDrop),
    ("TcpConnect", SystemOp::TcpConnect),
    ("TcpRead", SystemOp::TcpRead),
    ("TcpWrite", SystemOp::TcpWrite),
    ("TcpClose", SystemOp::TcpClose),
    ("TcpDrop", SystemOp::TcpDrop),
    ("TcpListenerBind", SystemOp::TcpListenerBind),
    ("TcpAccept", SystemOp::TcpAccept),
    ("TcpListenerClose", SystemOp::TcpListenerClose),
    ("TcpListenerDrop", SystemOp::TcpListenerDrop),
    ("DnsResolve", SystemOp::DnsResolve),
    ("UdpBind", SystemOp::UdpBind),
    ("UdpConnect", SystemOp::UdpConnect),
    ("UdpRead", SystemOp::UdpRead),
    ("UdpWrite", SystemOp::UdpWrite),
    ("UdpClose", SystemOp::UdpClose),
    ("UdpDrop", SystemOp::UdpDrop),
    ("ThreadSpawn", SystemOp::ThreadSpawn),
    ("ThreadJoin", SystemOp::ThreadJoin),
    ("ThreadId", SystemOp::ThreadId),
    ("ThreadDrop", SystemOp::ThreadDrop),
    ("KeyDown", SystemOp::KeyDown),
    ("KeyPressed", SystemOp::KeyPressed),
    ("KeyReleased", SystemOp::KeyReleased),
    ("ReadKey", SystemOp::ReadKey),
];

fn vec_op_name(value: VecOp) -> &'static str {
    match value {
        VecOp::New => "New",
        VecOp::Clone => "Clone",
        VecOp::Drop => "Drop",
        VecOp::Len => "Len",
        VecOp::Capacity => "Capacity",
        VecOp::Reserve => "Reserve",
        VecOp::Clear => "Clear",
        VecOp::Push => "Push",
        VecOp::Take => "Take",
        VecOp::Index => "Index",
        VecOp::Set => "Set",
        VecOp::Extract => "Extract",
        VecOp::IsEmpty => "IsEmpty",
        VecOp::Insert => "Insert",
        VecOp::Reverse => "Reverse",
        VecOp::Sort => "Sort",
        VecOp::Contains => "Contains",
        VecOp::GetOption => "GetOption",
        VecOp::PopOption => "PopOption",
    }
}

const VEC_OP_NAMES: &[(&str, VecOp)] = &[
    ("New", VecOp::New),
    ("Clone", VecOp::Clone),
    ("Drop", VecOp::Drop),
    ("Len", VecOp::Len),
    ("Capacity", VecOp::Capacity),
    ("Reserve", VecOp::Reserve),
    ("Clear", VecOp::Clear),
    ("Push", VecOp::Push),
    ("Take", VecOp::Take),
    ("Index", VecOp::Index),
    ("Set", VecOp::Set),
    ("Extract", VecOp::Extract),
    ("IsEmpty", VecOp::IsEmpty),
    ("Insert", VecOp::Insert),
    ("Reverse", VecOp::Reverse),
    ("Sort", VecOp::Sort),
    ("Contains", VecOp::Contains),
    ("GetOption", VecOp::GetOption),
    ("PopOption", VecOp::PopOption),
];

fn map_op_name(value: MapOp) -> &'static str {
    match value {
        MapOp::New => "New",
        MapOp::Drop => "Drop",
        MapOp::Clone => "Clone",
        MapOp::Len => "Len",
        MapOp::Clear => "Clear",
        MapOp::ContainsKey => "ContainsKey",
        MapOp::Insert => "Insert",
        MapOp::Get => "Get",
        MapOp::Remove => "Remove",
        MapOp::Keys => "Keys",
        MapOp::Values => "Values",
        MapOp::IsEmpty => "IsEmpty",
    }
}

const MAP_OP_NAMES: &[(&str, MapOp)] = &[
    ("New", MapOp::New),
    ("Drop", MapOp::Drop),
    ("Clone", MapOp::Clone),
    ("Len", MapOp::Len),
    ("Clear", MapOp::Clear),
    ("ContainsKey", MapOp::ContainsKey),
    ("Insert", MapOp::Insert),
    ("Get", MapOp::Get),
    ("Remove", MapOp::Remove),
    ("Keys", MapOp::Keys),
    ("Values", MapOp::Values),
    ("IsEmpty", MapOp::IsEmpty),
];

fn enum_predicate_name(value: EnumPredicate) -> &'static str {
    match value {
        EnumPredicate::IsSome => "IsSome",
        EnumPredicate::IsNone => "IsNone",
        EnumPredicate::IsOk => "IsOk",
        EnumPredicate::IsErr => "IsErr",
    }
}

const ENUM_PREDICATE_NAMES: &[(&str, EnumPredicate)] = &[
    ("IsSome", EnumPredicate::IsSome),
    ("IsNone", EnumPredicate::IsNone),
    ("IsOk", EnumPredicate::IsOk),
    ("IsErr", EnumPredicate::IsErr),
];

fn binary_op_name(value: BinaryOp) -> &'static str {
    match value {
        BinaryOp::Add => "Add",
        BinaryOp::Sub => "Sub",
        BinaryOp::Mul => "Mul",
        BinaryOp::Div => "Div",
        BinaryOp::Rem => "Rem",
        BinaryOp::Eq => "Eq",
        BinaryOp::Ne => "Ne",
        BinaryOp::Lt => "Lt",
        BinaryOp::Le => "Le",
        BinaryOp::Gt => "Gt",
        BinaryOp::Ge => "Ge",
        BinaryOp::BitAnd => "BitAnd",
        BinaryOp::BitXor => "BitXor",
        BinaryOp::BitOr => "BitOr",
        BinaryOp::ShiftLeft => "ShiftLeft",
        BinaryOp::ShiftRight => "ShiftRight",
        BinaryOp::And => "And",
        BinaryOp::Or => "Or",
    }
}

const BINARY_OP_NAMES: &[(&str, BinaryOp)] = &[
    ("Add", BinaryOp::Add),
    ("Sub", BinaryOp::Sub),
    ("Mul", BinaryOp::Mul),
    ("Div", BinaryOp::Div),
    ("Rem", BinaryOp::Rem),
    ("Eq", BinaryOp::Eq),
    ("Ne", BinaryOp::Ne),
    ("Lt", BinaryOp::Lt),
    ("Le", BinaryOp::Le),
    ("Gt", BinaryOp::Gt),
    ("Ge", BinaryOp::Ge),
    ("BitAnd", BinaryOp::BitAnd),
    ("BitXor", BinaryOp::BitXor),
    ("BitOr", BinaryOp::BitOr),
    ("ShiftLeft", BinaryOp::ShiftLeft),
    ("ShiftRight", BinaryOp::ShiftRight),
    ("And", BinaryOp::And),
    ("Or", BinaryOp::Or),
];

