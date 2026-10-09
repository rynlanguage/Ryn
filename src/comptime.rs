//! Compile-time evaluation.
//!
//! Ryn evaluates three things before semantic analysis, working on the
//! syntax tree so every frontend shares the result:
//!
//! * top-level `const NAME: Type = expression` items, folded into their uses;
//! * `comptime(expression)`, which forces evaluation of any expression made
//!   of literals, constants, and calls to ordinary functions;
//! * `static_assert(condition)` and `static_assert(condition, "message")`
//!   statements, which stop compilation when the condition is false.
//!
//! The metadata functions `field_count::<S>()`, `variant_count::<E>()`,
//! `type_name::<T>()`, and `has_field::<S>("name")` read the declarations of
//! the program, and `sizeof`/`alignof` of scalar, array, and plain structure
//! types is folded when a constant needs it.
//!
//! The evaluator runs a pure subset of the language: integers, floats,
//! booleans, characters, and string literals, local variables, `when`,
//! `while`, `for` over ranges, `return`, and calls. A step budget and a
//! recursion limit keep evaluation bounded and deterministic.

use std::collections::{HashMap, HashSet};

use crate::{
    ast::{
        BinaryOp, EnumDef, Expression, Function, Program, Statement, StructDef, TypeName, UseDecl,
    },
    source::{Diagnostic, Span},
    visit::{children_mut, visit_expression},
};

/// The marker stored in `Function::external_symbol` for a top-level constant.
pub const CONSTANT_MARKER: &str = "$const";

const STEP_LIMIT: u64 = 2_000_000;
const DEPTH_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntType {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntType {
    fn from_type(ty: &TypeName) -> Option<Self> {
        Some(match ty {
            TypeName::I8 => Self::I8,
            TypeName::I16 => Self::I16,
            TypeName::I32 => Self::I32,
            TypeName::I64 => Self::I64,
            TypeName::U8 => Self::U8,
            TypeName::U16 => Self::U16,
            TypeName::U32 => Self::U32,
            TypeName::U64 => Self::U64,
            _ => return None,
        })
    }

    fn type_name(self) -> TypeName {
        match self {
            Self::I8 => TypeName::I8,
            Self::I16 => TypeName::I16,
            Self::I32 => TypeName::I32,
            Self::I64 => TypeName::I64,
            Self::U8 => TypeName::U8,
            Self::U16 => TypeName::U16,
            Self::U32 => TypeName::U32,
            Self::U64 => TypeName::U64,
        }
    }

    fn bits(self) -> u32 {
        match self {
            Self::I8 | Self::U8 => 8,
            Self::I16 | Self::U16 => 16,
            Self::I32 | Self::U32 => 32,
            Self::I64 | Self::U64 => 64,
        }
    }

    fn signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    fn bounds(self) -> (i128, i128) {
        if self.signed() {
            (
                -(1i128 << (self.bits() - 1)),
                (1i128 << (self.bits() - 1)) - 1,
            )
        } else {
            (0, (1i128 << self.bits()) - 1)
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
        }
    }

    /// Wraps `value` into the range of the type, as an `as` cast does.
    fn wrap(self, value: i128) -> i128 {
        let modulus = 1i128 << self.bits();
        let mut wrapped = value.rem_euclid(modulus);
        if self.signed() && wrapped > self.bounds().1 {
            wrapped -= modulus;
        }
        wrapped
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FloatType {
    F32,
    F64,
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    /// An integer; `None` is a literal that has not met a typed operand yet.
    Int(i128, Option<IntType>),
    Float(f64, Option<FloatType>),
    Bool(bool),
    Char(char),
    Str(String),
    /// An owned `String`.
    Owned(String),
    Struct(String, Vec<(String, Value)>),
    Vec(Vec<Value>),
}

impl Value {
    fn describe(&self) -> &'static str {
        match self {
            Self::Int(..) => "an integer",
            Self::Float(..) => "a float",
            Self::Bool(_) => "a bool",
            Self::Char(_) => "a char",
            Self::Str(_) => "a string",
            Self::Owned(_) => "a String",
            Self::Struct(..) => "a structure",
            Self::Vec(_) => "a Vec",
        }
    }
}

fn error(code: &'static str, message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic {
        code,
        message: message.into(),
        span,
        help: None,
    }
}

fn not_constant(what: &str, span: Span) -> Diagnostic {
    Diagnostic {
        code: "R0461",
        message: format!("{what} cannot be evaluated at compile time"),
        span,
        help: Some(
            "compile-time code may use literals, constants, locals declared in the same function, and calls to functions made of the same constructs"
                .into(),
        ),
    }
}

enum Flow {
    Normal,
    Return(Option<Value>),
    Break,
    Continue,
}

struct Frame {
    scopes: Vec<HashMap<String, (Value, bool)>>,
    module_path: String,
    function_name: String,
}

struct Evaluator<'a> {
    functions: &'a [Function],
    by_name: HashMap<&'a str, usize>,
    structs: &'a [StructDef],
    enums: &'a [EnumDef],
    /// `alias::NAME` -> (constant, public) for every `use` of a module that declares constants.
    aliases: HashMap<String, (usize, bool)>,
    constants: HashMap<String, Value>,
    in_progress: HashSet<String>,
    steps: u64,
    depth: usize,
}

impl<'a> Evaluator<'a> {
    fn new(
        functions: &'a [Function],
        structs: &'a [StructDef],
        enums: &'a [EnumDef],
        uses: &[UseDecl],
    ) -> Self {
        let mut by_name = HashMap::new();
        for (index, function) in functions.iter().enumerate() {
            by_name.entry(function.name.as_str()).or_insert(index);
        }
        let mut aliases = HashMap::new();
        for import in uses {
            let Some(alias) = import.local_name() else {
                continue;
            };
            let module_path = import.path.join("::");
            let prefix = format!("{module_path}::");
            for (index, function) in functions.iter().enumerate() {
                if function.external_symbol.as_deref() == Some(CONSTANT_MARKER)
                    && function.module_path == module_path
                    && let Some(suffix) = function.name.strip_prefix(&prefix)
                {
                    aliases.insert(format!("{alias}::{suffix}"), (index, function.public));
                }
            }
        }
        Self {
            functions,
            by_name,
            structs,
            enums,
            aliases,
            constants: HashMap::new(),
            in_progress: HashSet::new(),
            steps: 0,
            depth: 0,
        }
    }

    fn tick(&mut self, span: Span) -> Result<(), Diagnostic> {
        self.steps += 1;
        if self.steps > STEP_LIMIT {
            return Err(error(
                "R0463",
                format!("compile-time evaluation exceeded the limit of {STEP_LIMIT} steps"),
                span,
            ));
        }
        Ok(())
    }

    /// Finds a function by the name written at a use, preferring the caller's module.
    fn resolve(&self, name: &str, module_path: &str) -> Option<usize> {
        if !module_path.is_empty() {
            let qualified = format!("{module_path}::{name}");
            if let Some(index) = self.by_name.get(qualified.as_str()) {
                return Some(*index);
            }
        }
        self.by_name.get(name).copied()
    }

    fn resolve_constant(
        &self,
        name: &str,
        module_path: &str,
        span: Span,
    ) -> Result<Option<usize>, Diagnostic> {
        if let Some(index) = self.resolve(name, module_path)
            && self.functions[index].external_symbol.as_deref() == Some(CONSTANT_MARKER)
        {
            let function = &self.functions[index];
            if !function.public && function.module_path != module_path {
                return Err(Diagnostic {
                    code: "R0425",
                    message: format!("constant `{name}` is private to its module"),
                    span,
                    help: Some("mark the constant `pub` to use it from another module".into()),
                });
            }
            return Ok(Some(index));
        }
        match self.aliases.get(name) {
            Some((index, true)) => Ok(Some(*index)),
            Some((_, false)) => Err(Diagnostic {
                code: "R0425",
                message: format!("constant `{name}` is private to its module"),
                span,
                help: Some("mark the constant `pub` to use it from another module".into()),
            }),
            None => Ok(None),
        }
    }

    fn constant(&mut self, index: usize, span: Span) -> Result<Value, Diagnostic> {
        let functions = self.functions;
        let function = &functions[index];
        if let Some(value) = self.constants.get(&function.name) {
            return Ok(value.clone());
        }
        if !self.in_progress.insert(function.name.clone()) {
            return Err(error(
                "R0462",
                format!("constant `{}` depends on itself", function.name),
                span,
            ));
        }
        let name = function.name.clone();
        let module_path = function.module_path.clone();
        let declared = function.return_type.clone().unwrap_or(TypeName::I64);
        let Some(expression) = function.return_value.clone() else {
            return Err(not_constant("this constant", function.span));
        };
        let mut frame = Frame {
            scopes: vec![HashMap::new()],
            module_path,
            function_name: name.clone(),
        };
        let result = self
            .expression(&expression, &mut frame)
            .and_then(|value| coerce(value, &declared, expression_span(&expression)));
        self.in_progress.remove(&name);
        let value = result?;
        self.constants.insert(name, value.clone());
        Ok(value)
    }

    fn expression(
        &mut self,
        expression: &Expression,
        frame: &mut Frame,
    ) -> Result<Value, Diagnostic> {
        let span = expression_span(expression);
        self.tick(span)?;
        match expression {
            Expression::Integer(value, _) => Ok(Value::Int(i128::from(*value), None)),
            Expression::Float(value, _) => Ok(Value::Float(*value, None)),
            Expression::Boolean(value, _) => Ok(Value::Bool(*value)),
            Expression::Character(value, _) => Ok(Value::Char(*value)),
            Expression::String(value, _) => Ok(Value::Str(value.clone())),
            Expression::Name(name, span) => {
                for scope in frame.scopes.iter().rev() {
                    if let Some((value, _)) = scope.get(name) {
                        return Ok(value.clone());
                    }
                }
                if let Some(index) = self.resolve_constant(name, &frame.module_path, *span)? {
                    return self.constant(index, *span);
                }
                Err(not_constant(&format!("`{name}`"), *span))
            }
            Expression::EnumConstruct {
                enum_name,
                variant,
                arguments,
                span,
            } if arguments.is_empty() => {
                let name = format!("{enum_name}::{variant}");
                if let Some(index) = self.resolve_constant(&name, &frame.module_path, *span)? {
                    return self.constant(index, *span);
                }
                Err(not_constant(&format!("`{name}`"), *span))
            }
            Expression::Negate(inner, span) => match self.expression(inner, frame)? {
                Value::Int(value, ty) => checked_int(-value, ty, *span),
                Value::Float(value, ty) => Ok(Value::Float(-value, ty)),
                other => Err(error(
                    "R0464",
                    format!("cannot negate {}", other.describe()),
                    *span,
                )),
            },
            Expression::Not(inner, span) => match self.expression(inner, frame)? {
                Value::Bool(value) => Ok(Value::Bool(!value)),
                other => Err(error(
                    "R0464",
                    format!("`!` needs a bool, found {}", other.describe()),
                    *span,
                )),
            },
            Expression::BitNot(inner, span) => match self.expression(inner, frame)? {
                Value::Int(value, Some(ty)) => checked_int(ty.wrap(!value), Some(ty), *span),
                Value::Int(value, None) => Ok(Value::Int(!value, None)),
                other => Err(error(
                    "R0464",
                    format!("`~` needs an integer, found {}", other.describe()),
                    *span,
                )),
            },
            Expression::Binary {
                op,
                left,
                right,
                span,
            } => {
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    let Value::Bool(left_value) = self.expression(left, frame)? else {
                        return Err(error("R0464", "`&&` and `||` need bool operands", *span));
                    };
                    if (*op == BinaryOp::And) != left_value {
                        return Ok(Value::Bool(left_value));
                    }
                    let Value::Bool(right_value) = self.expression(right, frame)? else {
                        return Err(error("R0464", "`&&` and `||` need bool operands", *span));
                    };
                    return Ok(Value::Bool(right_value));
                }
                let left = self.expression(left, frame)?;
                let right = self.expression(right, frame)?;
                binary(*op, left, right, *span)
            }
            Expression::If {
                condition,
                then_value,
                else_value,
                span,
            } => match self.expression(condition, frame)? {
                Value::Bool(true) => self.expression(then_value, frame),
                Value::Bool(false) => self.expression(else_value, frame),
                other => Err(error(
                    "R0464",
                    format!(
                        "a `when` condition needs a bool, found {}",
                        other.describe()
                    ),
                    *span,
                )),
            },
            Expression::Cast(inner, ty, span) => {
                let value = self.expression(inner, frame)?;
                cast(value, ty, *span)
            }
            Expression::LayoutOf {
                ty,
                alignment,
                span,
            } => {
                let (size, align) = self
                    .layout(ty)
                    .ok_or_else(|| not_constant("the layout of this type", *span))?;
                Ok(Value::Int(
                    if *alignment { align } else { size } as i128,
                    Some(IntType::U64),
                ))
            }
            Expression::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => self.call(name, type_arguments, arguments, *span, frame),
            Expression::VecConstructor { .. } => Ok(Value::Vec(Vec::new())),
            Expression::StructLiteral { name, fields, span } => {
                let Some(definition) = self.structs.iter().find(|s| s.name == *name) else {
                    return Err(not_constant(&format!("the structure `{name}`"), *span));
                };
                let mut values = Vec::new();
                for declared in &definition.fields {
                    let Some((_, expression, field_span)) =
                        fields.iter().find(|(field, _, _)| *field == declared.name)
                    else {
                        return Err(error(
                            "R0464",
                            format!("field `{}` of `{name}` has no value", declared.name),
                            *span,
                        ));
                    };
                    let value = self.expression(expression, frame)?;
                    values.push((
                        declared.name.clone(),
                        coerce(value, &declared.ty, *field_span)?,
                    ));
                }
                Ok(Value::Struct(name.clone(), values))
            }
            Expression::Field {
                value, name, span, ..
            } => match self.expression(value, frame)? {
                Value::Struct(_, fields) => fields
                    .into_iter()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| error("R0464", format!("no field `{name}`"), *span)),
                other => Err(error(
                    "R0464",
                    format!("{} has no field `{name}`", other.describe()),
                    *span,
                )),
            },
            Expression::MethodCall {
                value,
                name,
                arguments,
                span,
                ..
            } => {
                let receiver = self.expression(value, frame)?;
                let mut values = Vec::new();
                for argument in arguments {
                    values.push(self.expression(argument, frame)?);
                }
                pure_method(receiver, name, values, *span)
            }
            other => Err(not_constant("this expression", expression_span(other))),
        }
    }

    fn call(
        &mut self,
        name: &str,
        type_arguments: &[TypeName],
        arguments: &[Expression],
        span: Span,
        frame: &mut Frame,
    ) -> Result<Value, Diagnostic> {
        if name == "comptime" && arguments.len() == 1 {
            return self.expression(&arguments[0], frame);
        }
        if name == "Vec" && arguments.is_empty() {
            return Ok(Value::Vec(Vec::new()));
        }
        if name == "String"
            && arguments.len() == 1
            && self.resolve(name, &frame.module_path).is_none()
        {
            return match self.expression(&arguments[0], frame)? {
                Value::Str(text) | Value::Owned(text) => Ok(Value::Owned(text)),
                Value::Int(value, _) => Ok(Value::Owned(value.to_string())),
                other => Err(error(
                    "R0464",
                    format!(
                        "`String` cannot be built from {} at compile time",
                        other.describe()
                    ),
                    span,
                )),
            };
        }
        if let Some(value) = self.metadata(name, type_arguments, arguments, span)? {
            return Ok(value);
        }
        let Some(index) = self.resolve(name, &frame.module_path) else {
            return Err(not_constant(&format!("a call to `{name}`"), span));
        };
        let functions = self.functions;
        let function = &functions[index];
        if function.extern_c
            || !function.type_parameters.is_empty()
            || function.parameters.len() != arguments.len()
        {
            return Err(not_constant(&format!("a call to `{name}`"), span));
        }
        let mut callee = Frame {
            scopes: vec![HashMap::new()],
            module_path: function.module_path.clone(),
            function_name: function.name.clone(),
        };
        for (parameter, argument) in function.parameters.iter().zip(arguments) {
            let value = self.expression(argument, frame)?;
            let value = coerce(value, &parameter.ty, expression_span(argument))?;
            callee.scopes[0].insert(parameter.name.clone(), (value, true));
        }
        if self.depth >= DEPTH_LIMIT {
            return Err(error(
                "R0463",
                format!(
                    "compile-time recursion in `{}` is deeper than {DEPTH_LIMIT} calls",
                    callee.function_name
                ),
                span,
            ));
        }
        self.depth += 1;
        let outcome = self.run_function(index, &mut callee);
        self.depth -= 1;
        let value = outcome?;
        let function = &functions[index];
        match (value, &function.return_type) {
            (Some(value), Some(ty)) => coerce(value, ty, span),
            (None, None) => Err(not_constant(
                &format!("a call to `{name}`, which returns no value"),
                span,
            )),
            (Some(_), None) | (None, Some(_)) => {
                Err(not_constant(&format!("a call to `{name}`"), span))
            }
        }
    }

    fn run_function(
        &mut self,
        index: usize,
        frame: &mut Frame,
    ) -> Result<Option<Value>, Diagnostic> {
        let functions = self.functions;
        let function = &functions[index];
        match self.block(&function.body, frame)? {
            Flow::Return(value) => return Ok(value),
            Flow::Normal => {}
            Flow::Break | Flow::Continue => {
                return Err(not_constant(
                    "`break` or `continue` outside a loop",
                    function.span,
                ));
            }
        }
        match &function.return_value {
            Some(expression) => Ok(Some(self.expression(expression, frame)?)),
            None => Ok(None),
        }
    }

    fn block(&mut self, statements: &[Statement], frame: &mut Frame) -> Result<Flow, Diagnostic> {
        frame.scopes.push(HashMap::new());
        let mut flow = Flow::Normal;
        for statement in statements {
            match self.statement(statement, frame) {
                Ok(Flow::Normal) => {}
                Ok(other) => {
                    flow = other;
                    break;
                }
                Err(diagnostic) => {
                    frame.scopes.pop();
                    return Err(diagnostic);
                }
            }
        }
        frame.scopes.pop();
        Ok(flow)
    }

    fn assign(frame: &mut Frame, name: &str, value: Value, span: Span) -> Result<(), Diagnostic> {
        for scope in frame.scopes.iter_mut().rev() {
            if let Some((slot, mutable)) = scope.get_mut(name) {
                if !*mutable {
                    return Err(error("R0204", format!("`{name}` is immutable"), span));
                }
                let value = match (&*slot, value) {
                    (Value::Int(_, Some(ty)), Value::Int(v, None)) => {
                        checked_int(v, Some(*ty), span)?
                    }
                    (Value::Float(_, Some(ty)), Value::Float(v, None)) => {
                        Value::Float(v, Some(*ty))
                    }
                    (_, other) => other,
                };
                *slot = value;
                return Ok(());
            }
        }
        Err(not_constant(&format!("assignment to `{name}`"), span))
    }

    fn statement(&mut self, statement: &Statement, frame: &mut Frame) -> Result<Flow, Diagnostic> {
        match statement {
            Statement::Let {
                name,
                mutable,
                annotation,
                value,
                span,
            } => {
                self.tick(*span)?;
                let mut result = self.expression(value, frame)?;
                if let Some(annotation) = annotation {
                    result = coerce(result, annotation, *span)?;
                }
                frame
                    .scopes
                    .last_mut()
                    .expect("a scope exists")
                    .insert(name.clone(), (result, *mutable));
                Ok(Flow::Normal)
            }
            Statement::Assign { name, value, span } => {
                let result = self.expression(value, frame)?;
                Self::assign(frame, name, result, *span)?;
                Ok(Flow::Normal)
            }
            Statement::CompoundAssign {
                name,
                op,
                value,
                span,
            } => {
                let current = self.expression(&Expression::Name(name.clone(), *span), frame)?;
                let operand = self.expression(value, frame)?;
                let result = binary(*op, current, operand, *span)?;
                Self::assign(frame, name, result, *span)?;
                Ok(Flow::Normal)
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                span,
            } => match self.expression(condition, frame)? {
                Value::Bool(true) => self.block(then_body, frame),
                Value::Bool(false) => self.block(else_body, frame),
                other => Err(error(
                    "R0464",
                    format!(
                        "a `when` condition needs a bool, found {}",
                        other.describe()
                    ),
                    *span,
                )),
            },
            Statement::While {
                condition,
                body,
                span,
            } => {
                loop {
                    self.tick(*span)?;
                    match self.expression(condition, frame)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) => break,
                        other => {
                            return Err(error(
                                "R0464",
                                format!(
                                    "a `while` condition needs a bool, found {}",
                                    other.describe()
                                ),
                                *span,
                            ));
                        }
                    }
                    match self.block(body, frame)? {
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Normal | Flow::Continue => {}
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::For {
                name,
                start,
                end,
                inclusive,
                body,
                span,
                ..
            } => {
                let first = self.expression(start, frame)?;
                let last = self.expression(end, frame)?;
                let (Value::Int(first, first_type), Value::Int(last, last_type)) = (first, last)
                else {
                    return Err(error("R0464", "a `for` range needs integer bounds", *span));
                };
                let ty = first_type.or(last_type);
                let mut current = first;
                while current < last || (*inclusive && current == last) {
                    self.tick(*span)?;
                    frame.scopes.push(HashMap::from([(
                        name.clone(),
                        (Value::Int(current, ty), false),
                    )]));
                    let flow = self.block(body, frame);
                    frame.scopes.pop();
                    match flow? {
                        Flow::Break => break,
                        Flow::Return(value) => return Ok(Flow::Return(value)),
                        Flow::Normal | Flow::Continue => {}
                    }
                    current += 1;
                }
                Ok(Flow::Normal)
            }
            Statement::Return { value, .. } => {
                let value = match value {
                    Some(expression) => Some(self.expression(expression, frame)?),
                    None => None,
                };
                Ok(Flow::Return(value))
            }
            Statement::Break(_) => Ok(Flow::Break),
            Statement::Continue(_) => Ok(Flow::Continue),
            Statement::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => {
                self.call(name, type_arguments, arguments, *span, frame)
                    .or_else(|diagnostic| {
                        // A call statement may discard a function that returns nothing.
                        if diagnostic.message.contains("returns no value") {
                            Ok(Value::Bool(false))
                        } else {
                            Err(diagnostic)
                        }
                    })?;
                Ok(Flow::Normal)
            }
            Statement::MethodCall {
                value,
                name,
                arguments,
                span,
            } => {
                let mut values = Vec::new();
                for argument in arguments {
                    values.push(self.expression(argument, frame)?);
                }
                let slot = Self::place_mut(frame, value)
                    .ok_or_else(|| not_constant("a method call on this expression", *span))?;
                mutate(slot, name, values, *span)?;
                Ok(Flow::Normal)
            }
            Statement::FieldAssign {
                object,
                fields,
                op,
                value,
                span,
            } => {
                let mut result = self.expression(value, frame)?;
                let path: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();
                if let Some(op) = op {
                    let mut read =
                        self.expression(&Expression::Name(object.clone(), *span), frame)?;
                    for step in &path {
                        read = match read {
                            Value::Struct(_, entries) => entries
                                .into_iter()
                                .find(|(field, _)| field == step)
                                .map(|(_, value)| value)
                                .ok_or_else(|| {
                                    error("R0464", format!("no field `{step}`"), *span)
                                })?,
                            other => {
                                return Err(error(
                                    "R0464",
                                    format!("{} has no field `{step}`", other.describe()),
                                    *span,
                                ));
                            }
                        };
                    }
                    result = binary(*op, read, result, *span)?;
                }
                let slot = Self::local_mut(frame, object)
                    .ok_or_else(|| not_constant(&format!("`{object}`"), *span))?;
                let mut target = slot;
                for step in &path {
                    let Value::Struct(_, entries) = target else {
                        return Err(error("R0464", format!("no field `{step}`"), *span));
                    };
                    target = entries
                        .iter_mut()
                        .find(|(field, _)| field == step)
                        .map(|(_, value)| value)
                        .ok_or_else(|| error("R0464", format!("no field `{step}`"), *span))?;
                }
                *target = match (&*target, result) {
                    (Value::Int(_, Some(ty)), Value::Int(v, None)) => {
                        checked_int(v, Some(*ty), *span)?
                    }
                    (_, other) => other,
                };
                Ok(Flow::Normal)
            }
            other => Err(not_constant("this statement", statement_span(other))),
        }
    }

    /// A local variable or a field path inside one, as a place that can change.
    fn place_mut<'f>(frame: &'f mut Frame, expression: &Expression) -> Option<&'f mut Value> {
        match expression {
            Expression::Name(name, _) => Self::local_mut(frame, name),
            Expression::Field { value, name, .. } => {
                let Value::Struct(_, entries) = Self::place_mut(frame, value)? else {
                    return None;
                };
                entries
                    .iter_mut()
                    .find(|(field, _)| field == name)
                    .map(|(_, value)| value)
            }
            _ => None,
        }
    }

    fn local_mut<'f>(frame: &'f mut Frame, name: &str) -> Option<&'f mut Value> {
        frame
            .scopes
            .iter_mut()
            .rev()
            .find_map(|scope| scope.get_mut(name))
            .map(|(value, _)| value)
    }

    fn metadata(
        &self,
        name: &str,
        type_arguments: &[TypeName],
        arguments: &[Expression],
        span: Span,
    ) -> Result<Option<Value>, Diagnostic> {
        if !matches!(
            name,
            "field_count" | "variant_count" | "type_name" | "has_field"
        ) {
            return Ok(None);
        }
        let [ty] = type_arguments else {
            return Err(error(
                "R0465",
                format!("`{name}` takes exactly one type argument, written `{name}::<Type>()`"),
                span,
            ));
        };
        let wanted = match ty {
            TypeName::Named(wanted, _) => Some(wanted.as_str()),
            _ => None,
        };
        let structure = wanted.and_then(|wanted| self.structs.iter().find(|s| s.name == wanted));
        match name {
            "type_name" => Ok(Some(Value::Str(type_label(ty, self.structs)))),
            "field_count" => {
                let Some(structure) = structure else {
                    return Err(error("R0465", "`field_count` needs a structure type", span));
                };
                Ok(Some(Value::Int(
                    structure.fields.len() as i128,
                    Some(IntType::U64),
                )))
            }
            "has_field" => {
                let Some(structure) = structure else {
                    return Err(error("R0465", "`has_field` needs a structure type", span));
                };
                let [Expression::String(field, _)] = arguments else {
                    return Err(error(
                        "R0465",
                        "`has_field` takes the field name as a string literal",
                        span,
                    ));
                };
                Ok(Some(Value::Bool(
                    structure
                        .fields
                        .iter()
                        .any(|candidate| candidate.name == *field),
                )))
            }
            _ => {
                let definition =
                    wanted.and_then(|wanted| self.enums.iter().find(|e| e.name == wanted));
                let Some(definition) = definition else {
                    return Err(error(
                        "R0465",
                        "`variant_count` needs an enumeration type declared in this program",
                        span,
                    ));
                };
                Ok(Some(Value::Int(
                    definition.variants.len() as i128,
                    Some(IntType::U64),
                )))
            }
        }
    }

    /// Size and alignment of types whose layout does not depend on the target's ABI details.
    fn layout(&self, ty: &TypeName) -> Option<(u64, u64)> {
        match ty {
            TypeName::I8 | TypeName::U8 | TypeName::Bool => Some((1, 1)),
            TypeName::I16 | TypeName::U16 => Some((2, 2)),
            TypeName::I32 | TypeName::U32 | TypeName::F32 | TypeName::Char => Some((4, 4)),
            TypeName::I64 | TypeName::U64 | TypeName::F64 => Some((8, 8)),
            TypeName::RawPointer(..) | TypeName::Reference(..) | TypeName::FunctionPointer(..) => {
                Some((8, 8))
            }
            TypeName::Array(element, length, _) => {
                let (size, align) = self.layout(element)?;
                Some((size * *length as u64, align))
            }
            TypeName::Named(name, _) => {
                let structure = self.structs.iter().find(|s| s.name == *name)?;
                let mut offset = 0u64;
                let mut align_max = 1u64;
                for field in &structure.fields {
                    let (size, align) = self.layout(&field.ty)?;
                    offset = offset.div_ceil(align) * align + size;
                    align_max = align_max.max(align);
                }
                Some((offset.div_ceil(align_max) * align_max, align_max))
            }
            _ => None,
        }
    }
}

/// The readable name of a type. Instances of generic structures are called `$RynStruct#N` inside
/// the compiler; their `display` text names the template and its arguments.
fn type_label(ty: &TypeName, structs: &[StructDef]) -> String {
    match ty {
        TypeName::I8 => "i8".into(),
        TypeName::I16 => "i16".into(),
        TypeName::I32 => "i32".into(),
        TypeName::I64 => "i64".into(),
        TypeName::U8 => "u8".into(),
        TypeName::U16 => "u16".into(),
        TypeName::U32 => "u32".into(),
        TypeName::U64 => "u64".into(),
        TypeName::F32 => "f32".into(),
        TypeName::F64 => "f64".into(),
        TypeName::Str => "str".into(),
        TypeName::OwnedString => "String".into(),
        TypeName::Char => "char".into(),
        TypeName::Bool => "bool".into(),
        TypeName::Named(name, _) => instance_label(name, structs),
        TypeName::Parameter(name, _) => name.clone(),
        TypeName::Vec(element, _) => format!("Vec<{}>", type_label(element, structs)),
        TypeName::Set(element, _) => format!("Set<{}>", type_label(element, structs)),
        TypeName::Map(key, value, _) => {
            format!(
                "Map<{}, {}>",
                type_label(key, structs),
                type_label(value, structs)
            )
        }
        TypeName::Array(element, length, _) => {
            format!("[{}; {length}]", type_label(element, structs))
        }
        TypeName::Slice(element, _) => format!("&[{}]", type_label(element, structs)),
        TypeName::Reference(inner, mutable, _) => {
            format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                type_label(inner, structs)
            )
        }
        TypeName::RawPointer(inner, _) => format!("*{}", type_label(inner, structs)),
        TypeName::FunctionPointer(..) => "fun".into(),
    }
}

fn instance_label(name: &str, structs: &[StructDef]) -> String {
    let Some(display) = structs
        .iter()
        .find(|definition| definition.name == name && !definition.display.is_empty())
        .map(|definition| definition.display.as_str())
    else {
        return name.to_owned();
    };
    // `Box<$RynStruct#0,i32>`: expand nested instance names and space the arguments.
    let mut label = String::new();
    let mut rest = display;
    while let Some(at) = rest.find("$RynStruct#") {
        label.push_str(&rest[..at].replace(',', ", "));
        let digits = rest[at + "$RynStruct#".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .count();
        let end = at + "$RynStruct#".len() + digits;
        label.push_str(&instance_label(&rest[at..end], structs));
        rest = &rest[end..];
    }
    label.push_str(&rest.replace(',', ", "));
    label
}

fn checked_int(value: i128, ty: Option<IntType>, span: Span) -> Result<Value, Diagnostic> {
    if let Some(ty) = ty {
        let (low, high) = ty.bounds();
        if value < low || value > high {
            return Err(error(
                "R0464",
                format!(
                    "{value} does not fit in `{}` during compile-time evaluation",
                    ty.name()
                ),
                span,
            ));
        }
    } else if value < i128::from(i64::MIN) || value > i128::from(u64::MAX) {
        return Err(error(
            "R0464",
            format!("{value} is out of range during compile-time evaluation"),
            span,
        ));
    }
    Ok(Value::Int(value, ty))
}

fn binary(op: BinaryOp, left: Value, right: Value, span: Span) -> Result<Value, Diagnostic> {
    use BinaryOp::*;
    match (left, right) {
        (Value::Int(a, ta), Value::Int(b, tb)) => {
            let ty = match (ta, tb) {
                (Some(x), Some(y)) if x != y => {
                    return Err(error(
                        "R0464",
                        format!(
                            "operands have different integer types `{}` and `{}`",
                            x.name(),
                            y.name()
                        ),
                        span,
                    ));
                }
                (Some(x), _) | (_, Some(x)) => Some(x),
                (None, None) => None,
            };
            match op {
                Add => checked_int(a + b, ty, span),
                Sub => checked_int(a - b, ty, span),
                Mul => checked_int(a.checked_mul(b).unwrap_or(i128::MAX), ty, span),
                Div | Rem => {
                    if b == 0 {
                        return Err(error(
                            "R0464",
                            "division by zero during compile-time evaluation",
                            span,
                        ));
                    }
                    checked_int(if op == Div { a / b } else { a % b }, ty, span)
                }
                Eq => Ok(Value::Bool(a == b)),
                Ne => Ok(Value::Bool(a != b)),
                Lt => Ok(Value::Bool(a < b)),
                Le => Ok(Value::Bool(a <= b)),
                Gt => Ok(Value::Bool(a > b)),
                Ge => Ok(Value::Bool(a >= b)),
                BitAnd => checked_int(a & b, ty, span),
                BitOr => checked_int(a | b, ty, span),
                BitXor => checked_int(a ^ b, ty, span),
                ShiftLeft | ShiftRight => {
                    let bits = ty.map_or(64, IntType::bits);
                    if !(0..i128::from(bits)).contains(&b) {
                        return Err(error(
                            "R0464",
                            "shift amount is out of range during compile-time evaluation",
                            span,
                        ));
                    }
                    let shifted = if op == ShiftLeft { a << b } else { a >> b };
                    checked_int(ty.map_or(shifted, |t| t.wrap(shifted)), ty, span)
                }
                And | Or => Err(error("R0464", "`&&` and `||` need bool operands", span)),
            }
        }
        (Value::Float(a, ta), Value::Float(b, tb)) => {
            let ty = ta.or(tb);
            let round = |value: f64| match ty {
                Some(FloatType::F32) => f64::from(value as f32),
                _ => value,
            };
            Ok(match op {
                Add => Value::Float(round(a + b), ty),
                Sub => Value::Float(round(a - b), ty),
                Mul => Value::Float(round(a * b), ty),
                Div => Value::Float(round(a / b), ty),
                Eq => Value::Bool(a == b),
                Ne => Value::Bool(a != b),
                Lt => Value::Bool(a < b),
                Le => Value::Bool(a <= b),
                Gt => Value::Bool(a > b),
                Ge => Value::Bool(a >= b),
                _ => {
                    return Err(error(
                        "R0464",
                        "this operator does not apply to floating-point values",
                        span,
                    ));
                }
            })
        }
        (Value::Bool(a), Value::Bool(b)) => Ok(match op {
            Eq => Value::Bool(a == b),
            Ne => Value::Bool(a != b),
            _ => {
                return Err(error(
                    "R0464",
                    "this operator does not apply to bool values",
                    span,
                ));
            }
        }),
        (Value::Char(a), Value::Char(b)) => Ok(match op {
            Eq => Value::Bool(a == b),
            Ne => Value::Bool(a != b),
            Lt => Value::Bool(a < b),
            Le => Value::Bool(a <= b),
            Gt => Value::Bool(a > b),
            Ge => Value::Bool(a >= b),
            _ => {
                return Err(error(
                    "R0464",
                    "this operator does not apply to char values",
                    span,
                ));
            }
        }),
        (Value::Str(a) | Value::Owned(a), Value::Str(b) | Value::Owned(b)) => Ok(match op {
            Eq => Value::Bool(a == b),
            Ne => Value::Bool(a != b),
            _ => {
                return Err(error(
                    "R0464",
                    "this operator does not apply to strings",
                    span,
                ));
            }
        }),
        (left, right) => Err(error(
            "R0464",
            format!(
                "operands have different types: {} and {}",
                left.describe(),
                right.describe()
            ),
            span,
        )),
    }
}

fn cast(value: Value, ty: &TypeName, span: Span) -> Result<Value, Diagnostic> {
    if let Some(target) = IntType::from_type(ty) {
        return match value {
            Value::Int(v, _) => Ok(Value::Int(target.wrap(v), Some(target))),
            Value::Bool(v) => Ok(Value::Int(i128::from(v), Some(target))),
            Value::Char(v) => Ok(Value::Int(
                target.wrap(i128::from(u32::from(v))),
                Some(target),
            )),
            Value::Float(v, _) => {
                let (low, high) = target.bounds();
                let truncated = v.trunc();
                let clamped = if truncated.is_nan() {
                    0
                } else if truncated <= low as f64 {
                    low
                } else if truncated >= high as f64 {
                    high
                } else {
                    truncated as i128
                };
                Ok(Value::Int(clamped, Some(target)))
            }
            other => Err(error(
                "R0464",
                format!("cannot cast {} to an integer", other.describe()),
                span,
            )),
        };
    }
    match (ty, value) {
        (TypeName::F32, Value::Int(v, _)) => {
            Ok(Value::Float(f64::from(v as f32), Some(FloatType::F32)))
        }
        (TypeName::F64, Value::Int(v, _)) => Ok(Value::Float(v as f64, Some(FloatType::F64))),
        (TypeName::F32, Value::Float(v, _)) => {
            Ok(Value::Float(f64::from(v as f32), Some(FloatType::F32)))
        }
        (TypeName::F64, Value::Float(v, _)) => Ok(Value::Float(v, Some(FloatType::F64))),
        (TypeName::Bool, Value::Bool(v)) => Ok(Value::Bool(v)),
        (TypeName::Char, Value::Char(v)) => Ok(Value::Char(v)),
        (TypeName::Char, Value::Int(v, _)) => char::from_u32(u32::try_from(v).unwrap_or(u32::MAX))
            .map(Value::Char)
            .ok_or_else(|| error("R0464", "this value is not a valid character", span)),
        (_, value) => Err(error(
            "R0464",
            format!(
                "cannot cast {} to this type at compile time",
                value.describe()
            ),
            span,
        )),
    }
}

/// Converts a value to the declared type of a constant, parameter, or result.
fn coerce(value: Value, ty: &TypeName, span: Span) -> Result<Value, Diagnostic> {
    if let Some(target) = IntType::from_type(ty) {
        return match value {
            Value::Int(v, None) => checked_int(v, Some(target), span),
            Value::Int(v, Some(have)) if have == target => Ok(Value::Int(v, Some(target))),
            Value::Int(_, Some(have)) => Err(error(
                "R0464",
                format!(
                    "expected `{}` but the value has type `{}`",
                    target.name(),
                    have.name()
                ),
                span,
            )),
            other => Err(error(
                "R0464",
                format!(
                    "expected `{}` but found {}",
                    target.name(),
                    other.describe()
                ),
                span,
            )),
        };
    }
    match (ty, value) {
        (TypeName::F32, Value::Float(v, _)) => {
            Ok(Value::Float(f64::from(v as f32), Some(FloatType::F32)))
        }
        (TypeName::F64, Value::Float(v, _)) => Ok(Value::Float(v, Some(FloatType::F64))),
        (TypeName::Bool, Value::Bool(v)) => Ok(Value::Bool(v)),
        (TypeName::Char, Value::Char(v)) => Ok(Value::Char(v)),
        (TypeName::Str, Value::Str(v)) => Ok(Value::Str(v)),
        (TypeName::OwnedString, Value::Str(v) | Value::Owned(v)) => Ok(Value::Owned(v)),
        (TypeName::Named(name, _), Value::Struct(have, fields)) if *name == have => {
            Ok(Value::Struct(have, fields))
        }
        (TypeName::Vec(..), Value::Vec(items)) => Ok(Value::Vec(items)),
        (_, value) => Err(error(
            "R0464",
            format!(
                "a compile-time value of this type cannot be {} here",
                value.describe()
            ),
            span,
        )),
    }
}

fn expression_span(expression: &Expression) -> Span {
    expression.span()
}

fn statement_span(statement: &Statement) -> Span {
    match statement {
        Statement::Let { span, .. }
        | Statement::Assign { span, .. }
        | Statement::IndexAssign { span, .. }
        | Statement::DereferenceAssign { span, .. }
        | Statement::FieldAssign { span, .. }
        | Statement::CompoundAssign { span, .. }
        | Statement::Print(_, span)
        | Statement::PrintTemplate(_, span)
        | Statement::Call { span, .. }
        | Statement::MethodCall { span, .. }
        | Statement::If { span, .. }
        | Statement::While { span, .. }
        | Statement::Defer { span, .. }
        | Statement::For { span, .. }
        | Statement::ForEach { span, .. }
        | Statement::Return { span, .. } => *span,
        Statement::Break(span) | Statement::Continue(span) => *span,
    }
}

/// The literal expression that stands for `value` in the rewritten program.
fn literal(value: &Value, span: Span) -> Result<Expression, Diagnostic> {
    Ok(match value {
        Value::Int(v, ty) => {
            let magnitude = Expression::Integer(v.unsigned_abs() as u64, span);
            let signed = if *v < 0 {
                Expression::Negate(Box::new(magnitude), span)
            } else {
                magnitude
            };
            match ty {
                Some(ty) => Expression::Cast(Box::new(signed), ty.type_name(), span),
                None => signed,
            }
        }
        Value::Float(v, ty) => {
            let literal = Expression::Float(v.abs(), span);
            let literal = if v.is_sign_negative() {
                Expression::Negate(Box::new(literal), span)
            } else {
                literal
            };
            match ty {
                Some(FloatType::F32) => Expression::Cast(Box::new(literal), TypeName::F32, span),
                Some(FloatType::F64) => Expression::Cast(Box::new(literal), TypeName::F64, span),
                None => literal,
            }
        }
        Value::Bool(v) => Expression::Boolean(*v, span),
        Value::Char(v) => Expression::Character(*v, span),
        Value::Str(v) => Expression::String(v.clone(), span),
        Value::Owned(v) => Expression::Call {
            name: "String".into(),
            type_arguments: Vec::new(),
            arguments: vec![Expression::String(v.clone(), span)],
            span,
        },
        Value::Struct(name, fields) => Expression::StructLiteral {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Ok((field.clone(), literal(value, span)?, span)))
                .collect::<Result<_, Diagnostic>>()?,
            span,
        },
        Value::Vec(_) => {
            return Err(error(
                "R0464",
                "a Vec cannot be a compile-time constant; build it at run time",
                span,
            ));
        }
    })
}

/// Methods that only read their receiver.
fn pure_method(
    receiver: Value,
    name: &str,
    arguments: Vec<Value>,
    span: Span,
) -> Result<Value, Diagnostic> {
    let unsupported = || not_constant(&format!("the method `{name}`"), span);
    match (receiver, name, arguments.as_slice()) {
        (Value::Str(text) | Value::Owned(text), "len", []) => {
            Ok(Value::Int(text.len() as i128, Some(IntType::U64)))
        }
        (Value::Str(text) | Value::Owned(text), "char_count", []) => {
            Ok(Value::Int(text.chars().count() as i128, Some(IntType::U64)))
        }
        (Value::Owned(text), "clone", []) => Ok(Value::Owned(text)),
        (
            Value::Str(text) | Value::Owned(text),
            "concat",
            [Value::Str(other) | Value::Owned(other)],
        ) => Ok(Value::Owned(format!("{text}{other}"))),
        (Value::Vec(items), "len", []) => Ok(Value::Int(items.len() as i128, Some(IntType::U64))),
        (Value::Vec(items), "clone", []) => Ok(Value::Vec(items)),
        (Value::Vec(items), "index", [Value::Int(position, _)]) => usize::try_from(*position)
            .ok()
            .and_then(|position| items.get(position).cloned())
            .ok_or_else(|| {
                error(
                    "R0464",
                    "Vec index is out of range during compile-time evaluation",
                    span,
                )
            }),
        _ => Err(unsupported()),
    }
}

/// Methods that change a local value in place.
fn mutate(
    slot: &mut Value,
    name: &str,
    arguments: Vec<Value>,
    span: Span,
) -> Result<(), Diagnostic> {
    match (slot, name, arguments.as_slice()) {
        (Value::Vec(items), "push", [value]) => items.push(value.clone()),
        (Value::Vec(items), "clear", []) => items.clear(),
        (Value::Vec(items), "set", [Value::Int(position, _), value]) => {
            let position = usize::try_from(*position).unwrap_or(usize::MAX);
            let Some(entry) = items.get_mut(position) else {
                return Err(error(
                    "R0464",
                    "Vec index is out of range during compile-time evaluation",
                    span,
                ));
            };
            *entry = value.clone();
        }
        (Value::Vec(items), "take", [Value::Int(position, _)]) => {
            let position = usize::try_from(*position).unwrap_or(usize::MAX);
            if position >= items.len() {
                return Err(error(
                    "R0464",
                    "Vec index is out of range during compile-time evaluation",
                    span,
                ));
            }
            items.remove(position);
        }
        (Value::Owned(text), "append", [Value::Str(other) | Value::Owned(other)]) => {
            text.push_str(other);
        }
        _ => return Err(not_constant(&format!("the method `{name}`"), span)),
    }
    Ok(())
}

fn declared_names(function: &Function) -> HashSet<String> {
    fn statements(list: &[Statement], names: &mut HashSet<String>) {
        for statement in list {
            match statement {
                Statement::Let { name, .. } => {
                    names.insert(name.clone());
                }
                Statement::For { name, body, .. } | Statement::ForEach { name, body, .. } => {
                    names.insert(name.clone());
                    statements(body, names);
                }
                Statement::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    statements(then_body, names);
                    statements(else_body, names);
                }
                Statement::While { body, .. } | Statement::Defer { body, .. } => {
                    statements(body, names);
                }
                _ => {}
            }
        }
    }
    let mut names: HashSet<String> = function
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    statements(&function.body, &mut names);
    names
}

fn choose_bindings(function: &mut Function, names: &mut HashSet<String>) {
    let mut collect = |expression: &mut Expression| -> Result<(), Diagnostic> {
        if let Expression::Choose { arms, .. } = expression {
            for arm in arms {
                names.extend(arm.bindings.iter().cloned());
            }
        }
        Ok(())
    };
    let _ = crate::visit::visit_statements(&mut function.body, &mut collect);
    if let Some(value) = &mut function.return_value {
        let _ = visit_expression(value, &mut collect);
    }
}

/// Constants are removed before analysis, so an assignment to one would otherwise be reported as
/// an unknown variable. A local or parameter of the same name takes precedence.
fn reject_constant_assignments(
    list: &[Statement],
    evaluator: &Evaluator<'_>,
    module_path: &str,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    for statement in list {
        let target = match statement {
            Statement::Assign { name, span, .. }
            | Statement::IndexAssign { name, span, .. }
            | Statement::CompoundAssign { name, span, .. } => Some((name, *span)),
            Statement::FieldAssign { object, span, .. } => Some((object, *span)),
            _ => None,
        };
        if let Some((name, span)) = target
            && !locals.contains(name.as_str())
            && evaluator
                .resolve_constant(name, module_path, span)?
                .is_some()
        {
            let mut diagnostic = error(
                "R0467",
                format!("cannot assign to the constant `{name}`"),
                span,
            );
            diagnostic.help = Some(
                "constants are fixed at compile time; copy it into a `mut` local first".into(),
            );
            return Err(diagnostic);
        }
        match statement {
            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                reject_constant_assignments(then_body, evaluator, module_path, locals)?;
                reject_constant_assignments(else_body, evaluator, module_path, locals)?;
            }
            Statement::While { body, .. }
            | Statement::Defer { body, .. }
            | Statement::For { body, .. }
            | Statement::ForEach { body, .. } => {
                reject_constant_assignments(body, evaluator, module_path, locals)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether a condition names a constant (and no local of the same name) or calls `comptime`.
fn mentions_constant(
    condition: &Expression,
    evaluator: &Evaluator<'_>,
    module_path: &str,
    locals: &HashSet<String>,
) -> bool {
    fn walk(
        expression: &mut Expression,
        evaluator: &Evaluator<'_>,
        module_path: &str,
        locals: &HashSet<String>,
        found: &mut bool,
        blocked: &mut bool,
    ) {
        match expression {
            Expression::Name(name, span) => {
                if locals.contains(name.as_str()) {
                    *blocked = true;
                } else if evaluator
                    .resolve_constant(name, module_path, *span)
                    .is_ok_and(|constant| constant.is_some())
                {
                    *found = true;
                }
            }
            Expression::Call { name, .. } if name == "comptime" => *found = true,
            _ => {}
        }
        for child in children_mut(expression) {
            walk(child, evaluator, module_path, locals, found, blocked);
        }
    }
    let mut copy = condition.clone();
    let (mut found, mut blocked) = (false, false);
    walk(
        &mut copy,
        evaluator,
        module_path,
        locals,
        &mut found,
        &mut blocked,
    );
    found && !blocked
}

/// Removes and checks `static_assert` statements in a statement list.
fn check_assertions(
    statements: &mut Vec<Statement>,
    evaluator: &mut Evaluator<'_>,
    frame: &mut Frame,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    let mut kept = Vec::with_capacity(statements.len());
    for mut statement in std::mem::take(statements) {
        match &mut statement {
            Statement::Call {
                name,
                arguments,
                span,
                ..
            } if name == "static_assert" => {
                let (condition, message) = match arguments.as_slice() {
                    [condition] => (condition, None),
                    [condition, Expression::String(message, _)] => (condition, Some(message)),
                    _ => {
                        return Err(error(
                            "R0461",
                            "`static_assert` takes a condition and an optional message literal",
                            *span,
                        ));
                    }
                };
                match evaluator.expression(condition, frame)? {
                    Value::Bool(true) => {}
                    Value::Bool(false) => {
                        let mut diagnostic = error(
                            "R0460",
                            match message {
                                Some(message) => format!("static assertion failed: {message}"),
                                None => "static assertion failed".to_owned(),
                            },
                            *span,
                        );
                        diagnostic.help =
                            Some("the condition was evaluated at compile time and is false".into());
                        return Err(diagnostic);
                    }
                    other => {
                        return Err(error(
                            "R0464",
                            format!("`static_assert` needs a bool, found {}", other.describe()),
                            *span,
                        ));
                    }
                }
                continue;
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                // `when` on a compile-time condition keeps only the branch that is taken, so
                // the other branch is never analyzed: conditional compilation.
                if mentions_constant(condition, evaluator, &frame.module_path, locals)
                    && let Ok(Value::Bool(taken)) = evaluator.expression(condition, frame)
                {
                    let mut branch = std::mem::take(if taken { then_body } else { else_body });
                    check_assertions(&mut branch, evaluator, frame, locals)?;
                    *then_body = branch;
                    else_body.clear();
                    *condition = Expression::Boolean(true, expression_span(condition));
                } else {
                    check_assertions(then_body, evaluator, frame, locals)?;
                    check_assertions(else_body, evaluator, frame, locals)?;
                }
            }
            Statement::While { body, .. }
            | Statement::Defer { body, .. }
            | Statement::For { body, .. }
            | Statement::ForEach { body, .. } => check_assertions(body, evaluator, frame, locals)?,
            _ => {}
        }
        kept.push(statement);
    }
    *statements = kept;
    Ok(())
}

/// Evaluates constants, `comptime(...)`, metadata functions, and static assertions.
pub fn evaluate(program: &mut Program) -> Result<(), Diagnostic> {
    let has_work = program.functions.iter().any(|function| {
        function.external_symbol.as_deref() == Some(CONSTANT_MARKER) || mentions_comptime(function)
    });
    if !has_work {
        return Ok(());
    }
    let snapshot = program.functions.clone();
    let structs = program.structs.clone();
    let enums = program.enums.clone();
    let mut evaluator = Evaluator::new(&snapshot, &structs, &enums, &program.uses);

    // Every constant is evaluated even when nothing uses it, so mistakes surface.
    for (index, function) in snapshot.iter().enumerate() {
        if function.external_symbol.as_deref() == Some(CONSTANT_MARKER) {
            let value = evaluator.constant(index, function.span)?;
            literal(&value, function.span)?;
        }
    }

    for function in &mut program.functions {
        if function.external_symbol.as_deref() == Some(CONSTANT_MARKER) {
            continue;
        }
        let mut names = declared_names(function);
        choose_bindings(function, &mut names);
        let module_path = function.module_path.clone();
        let function_name = function.name.clone();
        let mut frame = Frame {
            scopes: vec![HashMap::new()],
            module_path: module_path.clone(),
            function_name,
        };
        reject_constant_assignments(&function.body, &evaluator, &module_path, &names)?;
        check_assertions(&mut function.body, &mut evaluator, &mut frame, &names)?;
        // The expression form `when c { a } else { b }` is folded the same way, so its dead arm
        // is never analyzed either. Children are visited before parents, so nested ones fold first.
        let mut fold = |expression: &mut Expression| -> Result<(), Diagnostic> {
            if let Expression::If {
                condition,
                then_value,
                else_value,
                ..
            } = expression
                && mentions_constant(condition, &evaluator, &module_path, &names)
                && let Ok(Value::Bool(taken)) = evaluator.expression(condition, &mut frame)
            {
                let chosen = if taken { then_value } else { else_value };
                *expression = (**chosen).clone();
            }
            Ok(())
        };
        crate::visit::visit_statements(&mut function.body, &mut fold)?;
        if let Some(value) = &mut function.return_value {
            visit_expression(value, &mut fold)?;
        }
        let mut rewrite = |expression: &mut Expression| -> Result<(), Diagnostic> {
            let span = expression_span(expression);
            let replacement = match expression {
                Expression::Name(name, _) if !names.contains(name.as_str()) => evaluator
                    .resolve_constant(name, &module_path, span)?
                    .map(|index| evaluator.constant(index, span))
                    .transpose()?,
                Expression::EnumConstruct {
                    enum_name,
                    variant,
                    arguments,
                    ..
                } if arguments.is_empty() => {
                    let qualified = format!("{enum_name}::{variant}");
                    evaluator
                        .resolve_constant(&qualified, &module_path, span)?
                        .map(|index| evaluator.constant(index, span))
                        .transpose()?
                }
                Expression::Call {
                    name,
                    type_arguments,
                    arguments,
                    ..
                } if (name == "comptime" && arguments.len() == 1)
                    || matches!(
                        name.as_str(),
                        "field_count" | "variant_count" | "type_name" | "has_field"
                    ) =>
                {
                    let mut frame = Frame {
                        scopes: vec![HashMap::new()],
                        module_path: module_path.clone(),
                        function_name: String::new(),
                    };
                    let call = Expression::Call {
                        name: name.clone(),
                        type_arguments: type_arguments.clone(),
                        arguments: arguments.clone(),
                        span,
                    };
                    Some(evaluator.expression(&call, &mut frame)?)
                }
                _ => None,
            };
            if let Some(value) = replacement {
                *expression = literal(&value, span)?;
            }
            Ok(())
        };
        crate::visit::visit_statements(&mut function.body, &mut rewrite)?;
        if let Some(value) = &mut function.return_value {
            visit_expression(value, &mut rewrite)?;
        }
    }
    program
        .functions
        .retain(|function| function.external_symbol.as_deref() != Some(CONSTANT_MARKER));
    Ok(())
}

fn mentions_comptime(function: &Function) -> bool {
    fn expression_has(expression: &mut Expression) -> bool {
        if let Expression::Call { name, .. } = expression
            && (name == "comptime"
                || matches!(
                    name.as_str(),
                    "field_count" | "variant_count" | "type_name" | "has_field"
                ))
        {
            return true;
        }
        children_mut(expression).into_iter().any(expression_has)
    }
    // `static_assert` statements are found by a walk of the statements; every expression of
    // every statement (including `when`, `while`, and `for` heads) is then checked for `comptime`.
    fn has_assertion(list: &[Statement]) -> bool {
        list.iter().any(|statement| match statement {
            Statement::Call { name, .. } if name == "static_assert" => true,
            Statement::If {
                then_body,
                else_body,
                ..
            } => has_assertion(then_body) || has_assertion(else_body),
            Statement::While { body, .. }
            | Statement::Defer { body, .. }
            | Statement::For { body, .. }
            | Statement::ForEach { body, .. } => has_assertion(body),
            _ => false,
        })
    }
    fn statements_have(list: &mut [Statement]) -> bool {
        let mut found = has_assertion(list);
        let mut probe = |expression: &mut Expression| -> Result<(), Diagnostic> {
            found |= expression_has(expression);
            Ok(())
        };
        let _ = crate::visit::visit_statements(list, &mut probe);
        found
    }
    let mut clone = function.clone();
    let mut found = statements_have(&mut clone.body);
    if let Some(value) = &mut clone.return_value {
        found |= expression_has(value);
    }
    found
}

/// Synthesizes `Type::default()` for `#[derive(Default)]` structures.
pub fn derive_defaults(program: &mut Program) -> Result<(), Diagnostic> {
    let span = Span::default();
    let mut generated = Vec::new();
    for definition in &program.structs {
        if !definition.derives.iter().any(|name| name == "Default") {
            continue;
        }
        let name = format!("{}::default", definition.name);
        if program
            .functions
            .iter()
            .any(|function| function.name == name)
        {
            continue;
        }
        let mut fields = Vec::new();
        for field in &definition.fields {
            let value = default_value(&field.ty, program, field.span)?;
            fields.push((field.name.clone(), value, field.span));
        }
        generated.push(Function {
            name,
            extern_c: false,
            external_symbol: None,
            type_parameters: Vec::new(),
            type_parameter_bounds: Vec::new(),
            public: true,
            module_path: definition.module_path.clone(),
            parameters: Vec::new(),
            return_type: Some(TypeName::Named(definition.name.clone(), span)),
            body: Vec::new(),
            return_value: Some(Expression::StructLiteral {
                name: definition.name.clone(),
                fields,
                span,
            }),
            span: definition.span,
        });
    }
    program.functions.extend(generated);
    Ok(())
}

fn default_value(ty: &TypeName, program: &Program, span: Span) -> Result<Expression, Diagnostic> {
    Ok(match ty {
        TypeName::I8
        | TypeName::I16
        | TypeName::I32
        | TypeName::I64
        | TypeName::U8
        | TypeName::U16
        | TypeName::U32
        | TypeName::U64 => {
            Expression::Cast(Box::new(Expression::Integer(0, span)), ty.clone(), span)
        }
        TypeName::F32 | TypeName::F64 => {
            Expression::Cast(Box::new(Expression::Float(0.0, span)), ty.clone(), span)
        }
        TypeName::Bool => Expression::Boolean(false, span),
        TypeName::Char => Expression::Character('\0', span),
        TypeName::OwnedString => Expression::Call {
            name: "String".into(),
            type_arguments: Vec::new(),
            arguments: vec![Expression::String(String::new(), span)],
            span,
        },
        TypeName::Str => Expression::String(String::new(), span),
        TypeName::Vec(element, _) => Expression::VecConstructor {
            element: (**element).clone(),
            span,
        },
        TypeName::Map(key, value, _) => Expression::MapConstructor {
            key: (**key).clone(),
            value: (**value).clone(),
            span,
        },
        TypeName::Set(element, _) => Expression::SetConstructor {
            element: (**element).clone(),
            span,
        },
        TypeName::Array(element, length, _) => Expression::ArrayRepeat {
            value: Box::new(default_value(element, program, span)?),
            length: *length as u64,
            span,
        },
        TypeName::Named(name, _)
            if program
                .structs
                .iter()
                .any(|s| s.name == *name && s.derives.iter().any(|d| d == "Default")) =>
        {
            Expression::EnumConstruct {
                enum_name: name.clone(),
                variant: "default".into(),
                arguments: Vec::new(),
                span,
            }
        }
        other => {
            return Err(error(
                "R0466",
                format!(
                    "`#[derive(Default)]` cannot build a default for a field of type `{}`",
                    type_label(other, &program.structs)
                ),
                span,
            ));
        }
    })
}
