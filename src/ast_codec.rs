//! Text encoding of the parsed [`Program`] exchanged with the self-hosted
//! frontend.
//!
//! The Ryn-written frontend (`selfhost/`) lexes and parses source text and
//! prints the resulting syntax tree in this format; the Rust compiler decodes
//! it and continues with semantic analysis, Ryn Guard, and code generation.
//! [`encode_program`] produces the same text from a Rust-parsed program, so the
//! two frontends can be compared byte for byte.
//!
//! The format is a prefix-order stream of words, each preceded by one space;
//! declarations and statements also start on a new line:
//!
//! - unsigned integers are decimal, booleans are `0` or `1`;
//! - strings are `<byte length>:<bytes>`, so they need no escaping;
//! - a span is two integers, `start end`;
//! - an optional value is `-`, or `+` followed by the value;
//! - a list is its length followed by the items;
//! - floats are written with Rust's shortest round-trip `Display` form;
//! - characters are their Unicode scalar value.
//!
//! A complete result is either `Ok <program>` or
//! `Error <code> <message> <span> <optional help>`.

use std::fmt::Write as _;

use crate::{
    ast::{
        BinaryOp, ChooseArm, ConstantDef, EnumDef, Expression, ExtendDef, Function, Parameter,
        PrintPart, Program, ShapeDef, ShapeMethod, Statement, StructDef, StructField, TypeAliasDef,
        TypeName, UseDecl, VariantDef,
    },
    source::{Diagnostic, Span},
};

/// Encodes a successful parse as `Ok <program>`.
pub fn encode_program(program: &Program) -> String {
    let mut encoder = Encoder::default();
    encoder.word("Ok");
    encoder.program(program);
    encoder.out
}

/// Encodes a parse failure as `Error <code> <message> <span> <help>`.
pub fn encode_diagnostic(diagnostic: &Diagnostic) -> String {
    let mut encoder = Encoder::default();
    encoder.word("Error");
    encoder.diagnostic_fields(diagnostic);
    encoder.out
}

/// Encodes the outcome of a recovering parse: the program, or
/// `Errors <count>` followed by each diagnostic.
pub fn encode_recovering_result(result: &Result<Program, Vec<Diagnostic>>) -> String {
    match result {
        Ok(program) => encode_program(program),
        Err(diagnostics) => {
            let mut encoder = Encoder::default();
            encoder.word("Errors");
            encoder.uint(diagnostics.len() as u64);
            for diagnostic in diagnostics {
                encoder.diagnostic_fields(diagnostic);
            }
            encoder.out
        }
    }
}

/// Decodes the output of a recovering parse.
pub fn decode_recovering_result(text: &str) -> Result<Program, Vec<Diagnostic>> {
    let mut decoder = Decoder { text, at: 0 };
    let outcome = (|| match decoder.word()? {
        "Ok" => Ok(Ok(decoder.program()?)),
        "Error" => Ok(Err(vec![decoder.diagnostic_fields()?])),
        "Errors" => Ok(Err(decoder.list(Decoder::diagnostic_fields)?)),
        other => Err(format!("unexpected result tag `{other}`")),
    })();
    match outcome {
        Ok(result) => {
            if let Err(error) = decoder.finish() {
                return Err(vec![internal(error)]);
            }
            result
        }
        Err(error) => Err(vec![internal(error)]),
    }
}

/// Encodes either outcome of a parse.
pub fn encode_result(result: &Result<Program, Diagnostic>) -> String {
    match result {
        Ok(program) => encode_program(program),
        Err(diagnostic) => encode_diagnostic(diagnostic),
    }
}

/// Decodes the frontend output. A malformed stream is reported as an internal
/// compiler error diagnostic rather than a panic.
pub fn decode_result(text: &str) -> Result<Program, Diagnostic> {
    let mut decoder = Decoder { text, at: 0 };
    let outcome = (|| match decoder.word()? {
        "Ok" => Ok(Ok(decoder.program()?)),
        "Error" => Ok(Err(decoder.diagnostic_fields()?)),
        other => Err(format!("unexpected result tag `{other}`")),
    })();
    match outcome {
        Ok(result) => {
            if let Err(error) = decoder.finish() {
                return Err(internal(error));
            }
            result
        }
        Err(error) => Err(internal(error)),
    }
}

fn internal(message: String) -> Diagnostic {
    Diagnostic {
        code: "R0901",
        message: format!("self-hosted frontend produced a malformed syntax tree: {message}"),
        span: Span::default(),
        help: Some("rebuild the frontend or use `--frontend rust`".into()),
    }
}

fn intern_code(code: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        "R0002", "R0003", "R0004", "R0005", "R0006", "R0007", "R0008", "R0009", "R0010", "R0012",
        "R0014", "R0015", "R0018", "R0019", "R0220", "R0221", "R0241", "R0242", "R0243", "R0244",
        "R0245", "R0260",
    ];
    KNOWN
        .iter()
        .copied()
        .find(|known| *known == code)
        .unwrap_or_else(|| Box::leak(code.to_owned().into_boxed_str()))
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

    fn line(&mut self) {
        self.out.push('\n');
    }

    fn diagnostic_fields(&mut self, diagnostic: &Diagnostic) {
        self.string(diagnostic.code);
        self.string(&diagnostic.message);
        self.span(diagnostic.span);
        self.option(diagnostic.help.as_ref(), |encoder, help| {
            encoder.string(help)
        });
    }

    fn uint(&mut self, value: u64) {
        self.word(&value.to_string());
    }

    fn boolean(&mut self, value: bool) {
        self.word(if value { "1" } else { "0" });
    }

    fn string(&mut self, value: &str) {
        let _ = write!(self.out, " {}:", value.len());
        self.out.push_str(value);
    }

    fn span(&mut self, span: Span) {
        self.uint(span.start as u64);
        self.uint(span.end as u64);
    }

    fn option<T>(&mut self, value: Option<T>, mut item: impl FnMut(&mut Self, T)) {
        match value {
            Some(value) => {
                self.word("+");
                item(self, value);
            }
            None => self.word("-"),
        }
    }

    fn list<'a, T: 'a>(
        &mut self,
        items: impl ExactSizeIterator<Item = &'a T>,
        mut item: impl FnMut(&mut Self, &'a T),
    ) {
        self.uint(items.len() as u64);
        for value in items {
            item(self, value);
        }
    }

    fn strings(&mut self, values: &[String]) {
        self.list(values.iter(), |encoder, value| encoder.string(value));
    }

    fn program(&mut self, program: &Program) {
        self.line();
        self.list(program.uses.iter(), Self::use_decl);
        self.line();
        self.list(program.structs.iter(), Self::struct_def);
        self.line();
        self.list(program.enums.iter(), Self::enum_def);
        self.line();
        self.list(program.type_aliases.iter(), Self::type_alias);
        self.line();
        self.list(program.extends.iter(), Self::extend_def);
        self.line();
        self.list(program.shapes.iter(), Self::shape_def);
        self.line();
        self.list(program.functions.iter(), Self::function);
        self.line();
    }

    fn use_decl(&mut self, declaration: &UseDecl) {
        self.line();
        self.strings(&declaration.path);
        self.span(declaration.span);
    }

    fn struct_def(&mut self, definition: &StructDef) {
        self.line();
        self.string(&definition.name);
        self.strings(&definition.type_parameters);
        self.list(definition.fields.iter(), Self::struct_field);
        self.boolean(definition.public);
        self.boolean(definition.repr_c);
        self.option(definition.drop_function.as_ref(), |encoder, name| {
            encoder.string(name);
        });
        self.strings(&definition.derives);
        self.string(&definition.module_path);
        self.span(definition.span);
    }

    fn struct_field(&mut self, field: &StructField) {
        self.string(&field.name);
        self.type_name(&field.ty);
        self.boolean(field.public);
        self.span(field.span);
    }

    fn enum_def(&mut self, definition: &EnumDef) {
        self.line();
        self.string(&definition.name);
        self.strings(&definition.type_parameters);
        self.list(definition.variants.iter(), Self::variant_def);
        self.boolean(definition.public);
        self.string(&definition.module_path);
        self.span(definition.span);
    }

    fn variant_def(&mut self, variant: &VariantDef) {
        self.string(&variant.name);
        self.list(variant.fields.iter(), Self::type_name);
        self.span(variant.span);
    }

    fn type_alias(&mut self, definition: &TypeAliasDef) {
        self.line();
        self.string(&definition.name);
        self.strings(&definition.type_parameters);
        self.type_name(&definition.ty);
        self.boolean(definition.public);
        self.string(&definition.module_path);
        self.span(definition.span);
    }

    fn extend_def(&mut self, definition: &ExtendDef) {
        self.line();
        self.type_name(&definition.type_name);
        self.option(definition.as_shape.as_ref(), |encoder, shape| {
            encoder.string(shape);
        });
        self.list(definition.functions.iter(), Self::function);
        self.list(definition.constants.iter(), Self::constant_def);
        self.string(&definition.module_path);
        self.span(definition.span);
    }

    fn constant_def(&mut self, constant: &ConstantDef) {
        self.string(&constant.name);
        self.type_name(&constant.ty);
        self.expression(&constant.value);
        self.boolean(constant.public);
        self.span(constant.span);
    }

    fn shape_def(&mut self, definition: &ShapeDef) {
        self.line();
        self.string(&definition.name);
        self.list(definition.methods.iter(), Self::shape_method);
        self.string(&definition.module_path);
        self.span(definition.span);
    }

    fn shape_method(&mut self, method: &ShapeMethod) {
        self.string(&method.name);
        self.list(method.parameters.iter(), Self::parameter);
        self.option(method.return_type.as_ref(), Self::type_name);
        self.option(method.default_body.as_ref(), |encoder, body| {
            encoder.statements(body);
        });
        self.option(method.default_value.as_ref(), Self::expression);
        self.span(method.span);
    }

    fn function(&mut self, function: &Function) {
        self.line();
        self.string(&function.name);
        self.boolean(function.extern_c);
        self.option(function.external_symbol.as_ref(), |encoder, symbol| {
            encoder.string(symbol);
        });
        self.strings(&function.type_parameters);
        self.list(
            function.type_parameter_bounds.iter(),
            |encoder, (parameter, bounds)| {
                encoder.string(parameter);
                encoder.strings(bounds);
            },
        );
        self.boolean(function.public);
        self.string(&function.module_path);
        self.list(function.parameters.iter(), Self::parameter);
        self.option(function.return_type.as_ref(), Self::type_name);
        self.statements(&function.body);
        self.option(function.return_value.as_ref(), Self::expression);
        self.span(function.span);
    }

    fn parameter(&mut self, parameter: &Parameter) {
        self.string(&parameter.name);
        self.type_name(&parameter.ty);
        self.span(parameter.span);
    }

    fn type_name(&mut self, ty: &TypeName) {
        match ty {
            TypeName::I8 => self.word("i8"),
            TypeName::I16 => self.word("i16"),
            TypeName::I32 => self.word("i32"),
            TypeName::I64 => self.word("i64"),
            TypeName::U8 => self.word("u8"),
            TypeName::U16 => self.word("u16"),
            TypeName::U32 => self.word("u32"),
            TypeName::U64 => self.word("u64"),
            TypeName::F32 => self.word("f32"),
            TypeName::F64 => self.word("f64"),
            TypeName::Str => self.word("str"),
            TypeName::OwnedString => self.word("String"),
            TypeName::Char => self.word("char"),
            TypeName::Bool => self.word("bool"),
            TypeName::Named(name, span) => {
                self.word("Named");
                self.string(name);
                self.span(*span);
            }
            TypeName::Parameter(name, span) => {
                self.word("Param");
                self.string(name);
                self.span(*span);
            }
            TypeName::Vec(element, span) => {
                self.word("Vec");
                self.type_name(element);
                self.span(*span);
            }
            TypeName::Map(key, value, span) => {
                self.word("Map");
                self.type_name(key);
                self.type_name(value);
                self.span(*span);
            }
            TypeName::Set(element, span) => {
                self.word("Set");
                self.type_name(element);
                self.span(*span);
            }
            TypeName::Array(element, length, span) => {
                self.word("Array");
                self.type_name(element);
                self.uint(*length as u64);
                self.span(*span);
            }
            TypeName::Slice(element, span) => {
                self.word("Slice");
                self.type_name(element);
                self.span(*span);
            }
            TypeName::Reference(element, mutable, span) => {
                self.word("Ref");
                self.type_name(element);
                self.boolean(*mutable);
                self.span(*span);
            }
            TypeName::RawPointer(element, span) => {
                self.word("Raw");
                self.type_name(element);
                self.span(*span);
            }
            TypeName::FunctionPointer(parameters, result, extern_c, span) => {
                self.word("FnPtr");
                self.list(parameters.iter(), Self::type_name);
                self.option(result.as_deref(), Self::type_name);
                self.boolean(*extern_c);
                self.span(*span);
            }
        }
    }

    fn statements(&mut self, statements: &[Statement]) {
        self.list(statements.iter(), Self::statement);
    }

    fn statement(&mut self, statement: &Statement) {
        self.line();
        match statement {
            Statement::Let {
                name,
                mutable,
                annotation,
                value,
                span,
            } => {
                self.word("Let");
                self.string(name);
                self.boolean(*mutable);
                self.option(annotation.as_ref(), Self::type_name);
                self.expression(value);
                self.span(*span);
            }
            Statement::Assign { name, value, span } => {
                self.word("Assign");
                self.string(name);
                self.expression(value);
                self.span(*span);
            }
            Statement::IndexAssign {
                name,
                index,
                value,
                span,
            } => {
                self.word("IndexAssign");
                self.string(name);
                self.expression(index);
                self.expression(value);
                self.span(*span);
            }
            Statement::DereferenceAssign {
                pointer,
                value,
                span,
            } => {
                self.word("DerefAssign");
                self.expression(pointer);
                self.expression(value);
                self.span(*span);
            }
            Statement::FieldAssign {
                object,
                fields,
                op,
                value,
                span,
            } => {
                self.word("FieldAssign");
                self.string(object);
                self.list(fields.iter(), |encoder, (field, span)| {
                    encoder.string(field);
                    encoder.span(*span);
                });
                self.option(op.as_ref(), |encoder, op| encoder.binary_op(*op));
                self.expression(value);
                self.span(*span);
            }
            Statement::CompoundAssign {
                name,
                op,
                value,
                span,
            } => {
                self.word("Compound");
                self.string(name);
                self.binary_op(*op);
                self.expression(value);
                self.span(*span);
            }
            Statement::Print(value, span) => {
                self.word("Print");
                self.expression(value);
                self.span(*span);
            }
            Statement::PrintTemplate(parts, span) => {
                self.word("Template");
                self.list(parts.iter(), |encoder, part| match part {
                    PrintPart::Text(text) => {
                        encoder.word("T");
                        encoder.string(text);
                    }
                    PrintPart::Value(value) => {
                        encoder.word("V");
                        encoder.expression(value);
                    }
                });
                self.span(*span);
            }
            Statement::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => {
                self.word("Call");
                self.string(name);
                self.list(type_arguments.iter(), Self::type_name);
                self.list(arguments.iter(), Self::expression);
                self.span(*span);
            }
            Statement::MethodCall {
                value,
                name,
                arguments,
                span,
            } => {
                self.word("MethodCall");
                self.expression(value);
                self.string(name);
                self.list(arguments.iter(), Self::expression);
                self.span(*span);
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                span,
            } => {
                self.word("If");
                self.expression(condition);
                self.statements(then_body);
                self.statements(else_body);
                self.span(*span);
            }
            Statement::While {
                condition,
                body,
                span,
            } => {
                self.word("While");
                self.expression(condition);
                self.statements(body);
                self.span(*span);
            }
            Statement::Defer { body, span } => {
                self.word("Defer");
                self.statements(body);
                self.span(*span);
            }
            Statement::For {
                name,
                name_span,
                start,
                end,
                inclusive,
                body,
                span,
            } => {
                // An inclusive range has its own tag, so the exclusive `For` layout is unchanged.
                self.word(if *inclusive { "ForInclusive" } else { "For" });
                self.string(name);
                self.span(*name_span);
                self.expression(start);
                self.expression(end);
                self.statements(body);
                self.span(*span);
            }
            Statement::ForEach {
                name,
                name_span,
                collection,
                body,
                span,
            } => {
                self.word("ForEach");
                self.string(name);
                self.span(*name_span);
                self.expression(collection);
                self.statements(body);
                self.span(*span);
            }
            Statement::Break(span) => {
                self.word("Break");
                self.span(*span);
            }
            Statement::Continue(span) => {
                self.word("Continue");
                self.span(*span);
            }
            Statement::Return { value, span } => {
                self.word("Return");
                self.option(value.as_ref(), Self::expression);
                self.span(*span);
            }
        }
    }

    fn binary_op(&mut self, op: BinaryOp) {
        self.word(binary_op_name(op));
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Integer(value, span) => {
                self.word("Int");
                self.uint(*value);
                self.span(*span);
            }
            Expression::Float(value, span) => {
                self.word("Float");
                self.word(&value.to_string());
                self.span(*span);
            }
            Expression::String(value, span) => {
                self.word("Str");
                self.string(value);
                self.span(*span);
            }
            Expression::Character(value, span) => {
                self.word("Char");
                self.uint(u64::from(u32::from(*value)));
                self.span(*span);
            }
            Expression::Boolean(value, span) => {
                self.word("Bool");
                self.boolean(*value);
                self.span(*span);
            }
            Expression::Name(name, span) => {
                self.word("Name");
                self.string(name);
                self.span(*span);
            }
            Expression::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => {
                self.word("Call");
                self.string(name);
                self.list(type_arguments.iter(), Self::type_name);
                self.list(arguments.iter(), Self::expression);
                self.span(*span);
            }
            Expression::MethodCall {
                value,
                name,
                type_arguments,
                arguments,
                span,
            } => {
                self.word("MethodCall");
                self.expression(value);
                self.string(name);
                self.list(type_arguments.iter(), Self::type_name);
                self.list(arguments.iter(), Self::expression);
                self.span(*span);
            }
            Expression::StructLiteral { name, fields, span } => {
                self.word("Struct");
                self.string(name);
                self.list(fields.iter(), |encoder, (field, value, span)| {
                    encoder.string(field);
                    encoder.expression(value);
                    encoder.span(*span);
                });
                self.span(*span);
            }
            Expression::Field {
                value,
                name,
                name_span,
                span,
            } => {
                self.word("Field");
                self.expression(value);
                self.string(name);
                self.span(*name_span);
                self.span(*span);
            }
            Expression::If {
                condition,
                then_value,
                else_value,
                span,
            } => {
                self.word("If");
                self.expression(condition);
                self.expression(then_value);
                self.expression(else_value);
                self.span(*span);
            }
            Expression::Binary {
                op,
                left,
                right,
                span,
            } => {
                self.word("Binary");
                self.binary_op(*op);
                self.expression(left);
                self.expression(right);
                self.span(*span);
            }
            Expression::Negate(value, span) => {
                self.word("Negate");
                self.expression(value);
                self.span(*span);
            }
            Expression::Not(value, span) => {
                self.word("Not");
                self.expression(value);
                self.span(*span);
            }
            Expression::BitNot(value, span) => {
                self.word("BitNot");
                self.expression(value);
                self.span(*span);
            }
            Expression::AddressOf {
                mutable,
                raw,
                value,
                span,
            } => {
                self.word("AddressOf");
                self.boolean(*mutable);
                self.boolean(*raw);
                self.expression(value);
                self.span(*span);
            }
            Expression::Dereference(value, span) => {
                self.word("Deref");
                self.expression(value);
                self.span(*span);
            }
            Expression::Cast(value, ty, span) => {
                self.word("Cast");
                self.expression(value);
                self.type_name(ty);
                self.span(*span);
            }
            Expression::LayoutOf {
                ty,
                alignment,
                span,
            } => {
                self.word("Layout");
                self.type_name(ty);
                self.boolean(*alignment);
                self.span(*span);
            }
            Expression::VecConstructor { element, span } => {
                self.word("NewVec");
                self.type_name(element);
                self.span(*span);
            }
            Expression::MapConstructor { key, value, span } => {
                self.word("NewMap");
                self.type_name(key);
                self.type_name(value);
                self.span(*span);
            }
            Expression::SetConstructor { element, span } => {
                self.word("NewSet");
                self.type_name(element);
                self.span(*span);
            }
            Expression::Tuple(values, span) => {
                self.word("Tuple");
                self.list(values.iter(), Self::expression);
                self.span(*span);
            }
            Expression::ArrayLiteral(values, span) => {
                self.word("Array");
                self.list(values.iter(), Self::expression);
                self.span(*span);
            }
            Expression::ArrayRepeat {
                value,
                length,
                span,
            } => {
                self.word("Repeat");
                self.expression(value);
                self.uint(*length);
                self.span(*span);
            }
            Expression::Index { value, index, span } => {
                self.word("Index");
                self.expression(value);
                self.expression(index);
                self.span(*span);
            }
            Expression::Propagate(value, span) => {
                self.word("Propagate");
                self.expression(value);
                self.span(*span);
            }
            Expression::EnumConstruct {
                enum_name,
                variant,
                arguments,
                span,
            } => {
                self.word("Enum");
                self.string(enum_name);
                self.string(variant);
                self.list(arguments.iter(), Self::expression);
                self.span(*span);
            }
            Expression::Choose { value, arms, span } => {
                self.word("Choose");
                self.expression(value);
                self.list(arms.iter(), Self::choose_arm);
                self.span(*span);
            }
        }
    }

    fn choose_arm(&mut self, arm: &ChooseArm) {
        self.option(arm.enum_name.as_ref(), |encoder, name| encoder.string(name));
        self.option(arm.variant.as_ref(), |encoder, name| encoder.string(name));
        self.strings(&arm.bindings);
        self.expression(&arm.body);
        self.span(arm.span);
    }
}

fn binary_op_name(op: BinaryOp) -> &'static str {
    match op {
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
        BinaryOp::ShiftLeft => "Shl",
        BinaryOp::ShiftRight => "Shr",
        BinaryOp::And => "And",
        BinaryOp::Or => "Or",
    }
}

type Decoded<T> = Result<T, String>;

struct Decoder<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Decoder<'a> {
    fn skip_space(&mut self) {
        let bytes = self.text.as_bytes();
        while self.at < bytes.len() && bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn diagnostic_fields(&mut self) -> Decoded<Diagnostic> {
        Ok(Diagnostic {
            code: intern_code(&self.string()?),
            message: self.string()?,
            span: self.span()?,
            help: self.option(Self::string)?,
        })
    }

    fn finish(&mut self) -> Decoded<()> {
        self.skip_space();
        if self.at == self.text.len() {
            Ok(())
        } else {
            Err(format!("trailing data at byte {}", self.at))
        }
    }

    fn word(&mut self) -> Decoded<&'a str> {
        self.skip_space();
        let bytes = self.text.as_bytes();
        let start = self.at;
        while self.at < bytes.len() && !bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
        if start == self.at {
            return Err("unexpected end of input".into());
        }
        Ok(&self.text[start..self.at])
    }

    fn uint(&mut self) -> Decoded<u64> {
        let word = self.word()?;
        word.parse()
            .map_err(|_| format!("expected an integer, found `{word}`"))
    }

    fn usize(&mut self) -> Decoded<usize> {
        let value = self.uint()?;
        usize::try_from(value).map_err(|_| format!("{value} exceeds the host size"))
    }

    fn boolean(&mut self) -> Decoded<bool> {
        match self.word()? {
            "0" => Ok(false),
            "1" => Ok(true),
            other => Err(format!("expected a boolean, found `{other}`")),
        }
    }

    fn string(&mut self) -> Decoded<String> {
        self.skip_space();
        let bytes = self.text.as_bytes();
        let start = self.at;
        while self.at < bytes.len() && bytes[self.at].is_ascii_digit() {
            self.at += 1;
        }
        if start == self.at || bytes.get(self.at) != Some(&b':') {
            return Err(format!("expected a string at byte {start}"));
        }
        let length: usize = self.text[start..self.at]
            .parse()
            .map_err(|_| "invalid string length".to_string())?;
        self.at += 1;
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.text.len())
            .ok_or_else(|| "string runs past the end of input".to_string())?;
        let value = self
            .text
            .get(self.at..end)
            .ok_or_else(|| "string splits a UTF-8 character".to_string())?;
        self.at = end;
        Ok(value.to_owned())
    }

    fn span(&mut self) -> Decoded<Span> {
        Ok(Span {
            start: self.usize()?,
            end: self.usize()?,
        })
    }

    fn option<T>(&mut self, mut item: impl FnMut(&mut Self) -> Decoded<T>) -> Decoded<Option<T>> {
        match self.word()? {
            "-" => Ok(None),
            "+" => Ok(Some(item(self)?)),
            other => Err(format!("expected `+` or `-`, found `{other}`")),
        }
    }

    fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Decoded<T>) -> Decoded<Vec<T>> {
        let count = self.usize()?;
        let mut items = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            items.push(item(self)?);
        }
        Ok(items)
    }

    fn strings(&mut self) -> Decoded<Vec<String>> {
        self.list(Self::string)
    }

    fn program(&mut self) -> Decoded<Program> {
        Ok(Program {
            uses: self.list(Self::use_decl)?,
            structs: self.list(Self::struct_def)?,
            enums: self.list(Self::enum_def)?,
            type_aliases: self.list(Self::type_alias)?,
            extends: self.list(Self::extend_def)?,
            shapes: self.list(Self::shape_def)?,
            functions: self.list(Self::function)?,
        })
    }

    fn use_decl(&mut self) -> Decoded<UseDecl> {
        Ok(UseDecl {
            path: self.strings()?,
            span: self.span()?,
        })
    }

    fn struct_def(&mut self) -> Decoded<StructDef> {
        Ok(StructDef {
            name: self.string()?,
            type_parameters: self.strings()?,
            fields: self.list(Self::struct_field)?,
            public: self.boolean()?,
            repr_c: self.boolean()?,
            drop_function: self.option(Self::string)?,
            derives: self.strings()?,
            module_path: self.string()?,
            span: self.span()?,
        })
    }

    fn struct_field(&mut self) -> Decoded<StructField> {
        Ok(StructField {
            name: self.string()?,
            ty: self.type_name()?,
            public: self.boolean()?,
            span: self.span()?,
        })
    }

    fn enum_def(&mut self) -> Decoded<EnumDef> {
        Ok(EnumDef {
            name: self.string()?,
            type_parameters: self.strings()?,
            variants: self.list(Self::variant_def)?,
            public: self.boolean()?,
            module_path: self.string()?,
            span: self.span()?,
        })
    }

    fn variant_def(&mut self) -> Decoded<VariantDef> {
        Ok(VariantDef {
            name: self.string()?,
            fields: self.list(Self::type_name)?,
            span: self.span()?,
        })
    }

    fn type_alias(&mut self) -> Decoded<TypeAliasDef> {
        Ok(TypeAliasDef {
            name: self.string()?,
            type_parameters: self.strings()?,
            ty: self.type_name()?,
            public: self.boolean()?,
            module_path: self.string()?,
            span: self.span()?,
        })
    }

    fn extend_def(&mut self) -> Decoded<ExtendDef> {
        Ok(ExtendDef {
            type_name: self.type_name()?,
            as_shape: self.option(Self::string)?,
            functions: self.list(Self::function)?,
            constants: self.list(Self::constant_def)?,
            module_path: self.string()?,
            span: self.span()?,
        })
    }

    fn constant_def(&mut self) -> Decoded<ConstantDef> {
        Ok(ConstantDef {
            name: self.string()?,
            ty: self.type_name()?,
            value: self.expression()?,
            public: self.boolean()?,
            span: self.span()?,
        })
    }

    fn shape_def(&mut self) -> Decoded<ShapeDef> {
        Ok(ShapeDef {
            name: self.string()?,
            methods: self.list(Self::shape_method)?,
            module_path: self.string()?,
            span: self.span()?,
        })
    }

    fn shape_method(&mut self) -> Decoded<ShapeMethod> {
        Ok(ShapeMethod {
            name: self.string()?,
            parameters: self.list(Self::parameter)?,
            return_type: self.option(Self::type_name)?,
            default_body: self.option(Self::statements)?,
            default_value: self.option(Self::expression)?,
            span: self.span()?,
        })
    }

    fn function(&mut self) -> Decoded<Function> {
        Ok(Function {
            name: self.string()?,
            extern_c: self.boolean()?,
            external_symbol: self.option(Self::string)?,
            type_parameters: self.strings()?,
            type_parameter_bounds: self
                .list(|decoder| Ok((decoder.string()?, decoder.strings()?)))?,
            public: self.boolean()?,
            module_path: self.string()?,
            parameters: self.list(Self::parameter)?,
            return_type: self.option(Self::type_name)?,
            body: self.statements()?,
            return_value: self.option(Self::expression)?,
            span: self.span()?,
        })
    }

    fn parameter(&mut self) -> Decoded<Parameter> {
        Ok(Parameter {
            name: self.string()?,
            ty: self.type_name()?,
            span: self.span()?,
        })
    }

    fn boxed_type(&mut self) -> Decoded<Box<TypeName>> {
        self.type_name().map(Box::new)
    }

    fn type_name(&mut self) -> Decoded<TypeName> {
        Ok(match self.word()? {
            "i8" => TypeName::I8,
            "i16" => TypeName::I16,
            "i32" => TypeName::I32,
            "i64" => TypeName::I64,
            "u8" => TypeName::U8,
            "u16" => TypeName::U16,
            "u32" => TypeName::U32,
            "u64" => TypeName::U64,
            "f32" => TypeName::F32,
            "f64" => TypeName::F64,
            "str" => TypeName::Str,
            "String" => TypeName::OwnedString,
            "char" => TypeName::Char,
            "bool" => TypeName::Bool,
            "Named" => TypeName::Named(self.string()?, self.span()?),
            "Param" => TypeName::Parameter(self.string()?, self.span()?),
            "Vec" => TypeName::Vec(self.boxed_type()?, self.span()?),
            "Map" => TypeName::Map(self.boxed_type()?, self.boxed_type()?, self.span()?),
            "Set" => TypeName::Set(self.boxed_type()?, self.span()?),
            "Array" => TypeName::Array(self.boxed_type()?, self.usize()?, self.span()?),
            "Slice" => TypeName::Slice(self.boxed_type()?, self.span()?),
            "Ref" => TypeName::Reference(self.boxed_type()?, self.boolean()?, self.span()?),
            "Raw" => TypeName::RawPointer(self.boxed_type()?, self.span()?),
            "FnPtr" => TypeName::FunctionPointer(
                self.list(Self::type_name)?,
                self.option(Self::boxed_type)?,
                self.boolean()?,
                self.span()?,
            ),
            other => return Err(format!("unknown type tag `{other}`")),
        })
    }

    fn statements(&mut self) -> Decoded<Vec<Statement>> {
        self.list(Self::statement)
    }

    fn binary_op(&mut self) -> Decoded<BinaryOp> {
        Ok(match self.word()? {
            "Add" => BinaryOp::Add,
            "Sub" => BinaryOp::Sub,
            "Mul" => BinaryOp::Mul,
            "Div" => BinaryOp::Div,
            "Rem" => BinaryOp::Rem,
            "Eq" => BinaryOp::Eq,
            "Ne" => BinaryOp::Ne,
            "Lt" => BinaryOp::Lt,
            "Le" => BinaryOp::Le,
            "Gt" => BinaryOp::Gt,
            "Ge" => BinaryOp::Ge,
            "BitAnd" => BinaryOp::BitAnd,
            "BitXor" => BinaryOp::BitXor,
            "BitOr" => BinaryOp::BitOr,
            "Shl" => BinaryOp::ShiftLeft,
            "Shr" => BinaryOp::ShiftRight,
            "And" => BinaryOp::And,
            "Or" => BinaryOp::Or,
            other => return Err(format!("unknown operator `{other}`")),
        })
    }

    fn statement(&mut self) -> Decoded<Statement> {
        Ok(match self.word()? {
            "Let" => Statement::Let {
                name: self.string()?,
                mutable: self.boolean()?,
                annotation: self.option(Self::type_name)?,
                value: self.expression()?,
                span: self.span()?,
            },
            "Assign" => Statement::Assign {
                name: self.string()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "IndexAssign" => Statement::IndexAssign {
                name: self.string()?,
                index: self.expression()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "DerefAssign" => Statement::DereferenceAssign {
                pointer: self.expression()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "FieldAssign" => Statement::FieldAssign {
                object: self.string()?,
                fields: self.list(|decoder| Ok((decoder.string()?, decoder.span()?)))?,
                op: self.option(Self::binary_op)?,
                value: self.expression()?,
                span: self.span()?,
            },
            "Compound" => Statement::CompoundAssign {
                name: self.string()?,
                op: self.binary_op()?,
                value: self.expression()?,
                span: self.span()?,
            },
            "Print" => Statement::Print(self.expression()?, self.span()?),
            "Template" => Statement::PrintTemplate(
                self.list(|decoder| match decoder.word()? {
                    "T" => Ok(PrintPart::Text(decoder.string()?)),
                    "V" => Ok(PrintPart::Value(decoder.expression()?)),
                    other => Err(format!("unknown template part `{other}`")),
                })?,
                self.span()?,
            ),
            "Call" => Statement::Call {
                name: self.string()?,
                type_arguments: self.list(Self::type_name)?,
                arguments: self.list(Self::expression)?,
                span: self.span()?,
            },
            "MethodCall" => Statement::MethodCall {
                value: self.boxed_expression()?,
                name: self.string()?,
                arguments: self.list(Self::expression)?,
                span: self.span()?,
            },
            "If" => Statement::If {
                condition: self.expression()?,
                then_body: self.statements()?,
                else_body: self.statements()?,
                span: self.span()?,
            },
            "While" => Statement::While {
                condition: self.expression()?,
                body: self.statements()?,
                span: self.span()?,
            },
            "Defer" => Statement::Defer {
                body: self.statements()?,
                span: self.span()?,
            },
            "For" => Statement::For {
                name: self.string()?,
                name_span: self.span()?,
                start: self.expression()?,
                end: self.expression()?,
                inclusive: false,
                body: self.statements()?,
                span: self.span()?,
            },
            "ForInclusive" => Statement::For {
                name: self.string()?,
                name_span: self.span()?,
                start: self.expression()?,
                end: self.expression()?,
                inclusive: true,
                body: self.statements()?,
                span: self.span()?,
            },
            "ForEach" => Statement::ForEach {
                name: self.string()?,
                name_span: self.span()?,
                collection: self.expression()?,
                body: self.statements()?,
                span: self.span()?,
            },
            "Break" => Statement::Break(self.span()?),
            "Continue" => Statement::Continue(self.span()?),
            "Return" => Statement::Return {
                value: self.option(Self::expression)?,
                span: self.span()?,
            },
            other => return Err(format!("unknown statement tag `{other}`")),
        })
    }

    fn boxed_expression(&mut self) -> Decoded<Box<Expression>> {
        self.expression().map(Box::new)
    }

    fn expression(&mut self) -> Decoded<Expression> {
        Ok(match self.word()? {
            "Int" => Expression::Integer(self.uint()?, self.span()?),
            "Float" => {
                let word = self.word()?;
                let value = word
                    .parse()
                    .map_err(|_| format!("invalid float `{word}`"))?;
                Expression::Float(value, self.span()?)
            }
            "Str" => Expression::String(self.string()?, self.span()?),
            "Char" => {
                let value = self.uint()?;
                let character = u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("invalid character {value}"))?;
                Expression::Character(character, self.span()?)
            }
            "Bool" => Expression::Boolean(self.boolean()?, self.span()?),
            "Name" => Expression::Name(self.string()?, self.span()?),
            "Call" => Expression::Call {
                name: self.string()?,
                type_arguments: self.list(Self::type_name)?,
                arguments: self.list(Self::expression)?,
                span: self.span()?,
            },
            "MethodCall" => Expression::MethodCall {
                value: self.boxed_expression()?,
                name: self.string()?,
                type_arguments: self.list(Self::type_name)?,
                arguments: self.list(Self::expression)?,
                span: self.span()?,
            },
            "Struct" => Expression::StructLiteral {
                name: self.string()?,
                fields: self.list(|decoder| {
                    Ok((decoder.string()?, decoder.expression()?, decoder.span()?))
                })?,
                span: self.span()?,
            },
            "Field" => Expression::Field {
                value: self.boxed_expression()?,
                name: self.string()?,
                name_span: self.span()?,
                span: self.span()?,
            },
            "If" => Expression::If {
                condition: self.boxed_expression()?,
                then_value: self.boxed_expression()?,
                else_value: self.boxed_expression()?,
                span: self.span()?,
            },
            "Binary" => Expression::Binary {
                op: self.binary_op()?,
                left: self.boxed_expression()?,
                right: self.boxed_expression()?,
                span: self.span()?,
            },
            // The self-hosted parser sends `|>` and `??` as these tags; they are rewritten
            // exactly as the bootstrap parser rewrites them, so both trees are identical.
            "Pipe" => {
                let value = self.expression()?;
                let target = self.expression()?;
                let span = self.span()?;
                Expression::pipe_into(value, target, span)
                    .map_err(|(_, message)| message)?
            }
            "Coalesce" => {
                let value = self.expression()?;
                let fallback = self.expression()?;
                let span = self.span()?;
                Expression::coalesce(value, fallback, span)
            }
            "Negate" => Expression::Negate(self.boxed_expression()?, self.span()?),
            "Not" => Expression::Not(self.boxed_expression()?, self.span()?),
            "BitNot" => Expression::BitNot(self.boxed_expression()?, self.span()?),
            "AddressOf" => Expression::AddressOf {
                mutable: self.boolean()?,
                raw: self.boolean()?,
                value: self.boxed_expression()?,
                span: self.span()?,
            },
            "Deref" => Expression::Dereference(self.boxed_expression()?, self.span()?),
            "Cast" => Expression::Cast(self.boxed_expression()?, self.type_name()?, self.span()?),
            "Layout" => Expression::LayoutOf {
                ty: self.type_name()?,
                alignment: self.boolean()?,
                span: self.span()?,
            },
            "NewVec" => Expression::VecConstructor {
                element: self.type_name()?,
                span: self.span()?,
            },
            "NewMap" => Expression::MapConstructor {
                key: self.type_name()?,
                value: self.type_name()?,
                span: self.span()?,
            },
            "NewSet" => Expression::SetConstructor {
                element: self.type_name()?,
                span: self.span()?,
            },
            "Tuple" => Expression::Tuple(self.list(Self::expression)?, self.span()?),
            "Array" => Expression::ArrayLiteral(self.list(Self::expression)?, self.span()?),
            "Repeat" => Expression::ArrayRepeat {
                value: self.boxed_expression()?,
                length: self.uint()?,
                span: self.span()?,
            },
            "Index" => Expression::Index {
                value: self.boxed_expression()?,
                index: self.boxed_expression()?,
                span: self.span()?,
            },
            "Propagate" => Expression::Propagate(self.boxed_expression()?, self.span()?),
            "Enum" => Expression::EnumConstruct {
                enum_name: self.string()?,
                variant: self.string()?,
                arguments: self.list(Self::expression)?,
                span: self.span()?,
            },
            "Choose" => Expression::Choose {
                value: self.boxed_expression()?,
                arms: self.list(Self::choose_arm)?,
                span: self.span()?,
            },
            other => return Err(format!("unknown expression tag `{other}`")),
        })
    }

    fn choose_arm(&mut self) -> Decoded<ChooseArm> {
        Ok(ChooseArm {
            enum_name: self.option(Self::string)?,
            variant: self.option(Self::string)?,
            bindings: self.strings()?,
            body: self.expression()?,
            span: self.span()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_result, encode_result};
    use crate::parser::parse;

    fn round_trip(source: &str) {
        let parsed = parse(source);
        let encoded = encode_result(&parsed);
        let decoded = decode_result(&encoded);
        assert_eq!(
            format!("{parsed:?}"),
            format!("{decoded:?}"),
            "round trip changed the tree for {source:?}"
        );
        assert_eq!(encode_result(&decoded), encoded);
    }

    #[test]
    fn round_trips_programs_and_diagnostics() {
        round_trip("fun main() { echo 42 }");
        round_trip(
            "struct Point { x: f64, y: f64 }\nfun main() { p := Point { x: 1.5, y: 2e10 } echo \"{p.x} and {p.y}\" }",
        );
        round_trip(
            "enum Shape { Dot, Box(i32, String) }\nfun size(s: Shape) -> i32 => choose s { Shape::Dot => 0, Shape::Box(w, _n) => w }\nfun main() {}",
        );
        round_trip("fun main() { mut v := Vec<Option<i32>>() v.push(Option::Some(1)) x := 'é' }");
        round_trip("fun main() { echo 1 +* }");
        round_trip("fun main() { echo \"unterminated }");
    }
}
