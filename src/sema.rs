use std::collections::{HashMap, HashSet};

use crate::{
    ast::{BinaryOp, Expression, Function, PrintPart, Program, Statement, StructDef, TypeName},
    source::{Diagnostic, Span},
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
    Bool,
    Struct(usize),
}

#[derive(Clone, Debug)]
pub enum IrExpression {
    Integer(i128, Type),
    Float(f64, Type),
    String(String),
    Boolean(bool),
    Local {
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
    Binary {
        op: BinaryOp,
        left: Box<IrExpression>,
        right: Box<IrExpression>,
        ty: Type,
    },
    Negate(Box<IrExpression>, Type),
    Not(Box<IrExpression>),
    BitNot(Box<IrExpression>, Type),
    Cast {
        value: Box<IrExpression>,
        source: Type,
        target: Type,
    },
}

#[derive(Clone, Debug)]
pub enum IrStatement {
    Let {
        slot: usize,
        ty: Type,
        value: IrExpression,
    },
    Assign {
        slot: usize,
        ty: Type,
        value: IrExpression,
    },
    FieldAssign {
        slot: usize,
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
        condition: IrExpression,
        body: Vec<IrStatement>,
    },
    For {
        slot: usize,
        end_slot: usize,
        ty: Type,
        start: IrExpression,
        end: IrExpression,
        body: Vec<IrStatement>,
    },
    Break,
    Continue,
    Return {
        value: Option<IrExpression>,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum IrCallTarget {
    Function(usize),
    ArgumentCount,
    Argument,
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
}

#[derive(Clone, Copy, Debug)]
pub struct LocalBinding {
    pub slot: usize,
    pub ty: Type,
}

#[derive(Debug)]
pub struct RynFunction {
    pub parameters: Vec<LocalBinding>,
    pub return_type: Option<Type>,
    pub return_value: Option<IrExpression>,
    pub statements: Vec<IrStatement>,
    pub local_types: Vec<LocalType>,
}

#[derive(Debug)]
pub struct RynIr {
    pub functions: Vec<RynFunction>,
    pub main_index: usize,
    pub structs: Vec<RynStruct>,
}

#[derive(Clone, Debug)]
pub struct RynStruct {
    pub name: String,
    pub fields: Vec<RynStructField>,
    pub slot_count: usize,
}

#[derive(Clone, Debug)]
pub struct RynStructField {
    pub name: String,
    pub ty: Type,
    pub slot_offset: usize,
}

#[derive(Clone, Copy)]
struct Binding {
    slot: usize,
    ty: Type,
    mutable: bool,
}

#[derive(Clone)]
struct FunctionSignature {
    target: IrCallTarget,
    parameters: Vec<Type>,
    return_type: Option<Type>,
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

fn analyze_with_recovery(program: Program, recover_errors: bool) -> Result<RynIr, Vec<Diagnostic>> {
    let mut struct_ids = HashMap::new();
    let mut declaration_errors = Vec::new();
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
        }
    }
    if recover_errors {
        let mut function_names = HashSet::new();
        for function in &program.functions {
            if !function_names.insert(function.name.as_str()) {
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

        let mut struct_errors = collect_struct_declaration_errors(&program.structs, &struct_ids);
        if !struct_errors.is_empty() {
            struct_errors.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(struct_errors);
        }

        let mut signature_errors =
            collect_function_signature_errors(&program.functions, &struct_ids);
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
    let structs =
        resolve_struct_layouts(&program.structs, &struct_ids).map_err(|error| vec![error])?;
    let mut signatures = HashMap::new();
    let mut main_index = None;
    for (index, function) in program.functions.iter().enumerate() {
        if signatures.contains_key(&function.name) {
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
            let mut parameters = Vec::with_capacity(function.parameters.len());
            for parameter in &function.parameters {
                parameters.push(resolve_type_name(&parameter.ty, &struct_ids)?);
            }
            let return_type = function
                .return_type
                .as_ref()
                .map(|name| resolve_type_name(name, &struct_ids))
                .transpose()?;
            if function.name == "main"
                && (!function.parameters.is_empty()
                    || return_type.is_some_and(|return_type| return_type != Type::I32))
            {
                return Err(diag(
                    "R0208",
                    "`main` must take no parameters and return either no value or `i32`",
                    function.span,
                )
                .with_help("use `fn main() { ... }` or `fn main() -> i32 { ... }`; move reusable work into another function"));
            }
            Ok(FunctionSignature {
                target: IrCallTarget::Function(index),
                parameters,
                return_type,
            })
        })();
        let signature = signature_result.map_err(|diagnostic| vec![diagnostic])?;
        if function.name == "main" {
            main_index = Some(index);
        }
        signatures.insert(function.name.clone(), signature);
    }
    let Some(main_index) = main_index else {
        return Err(vec![
            diag("R0200", "program must define `fn main()`", Span::default())
                .with_help("add a top-level `fn main() { ... }` function"),
        ]);
    };
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
        let analyzer = Analyzer::new(&signatures, &structs, &struct_ids);
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
    Ok(RynIr {
        functions,
        main_index,
        structs,
    })
}

struct Analyzer<'a> {
    names: HashMap<String, Binding>,
    local_types: Vec<LocalType>,
    signatures: &'a HashMap<String, FunctionSignature>,
    structs: &'a [RynStruct],
    struct_ids: &'a HashMap<String, usize>,
    loop_depth: usize,
    function_name: String,
    return_type: Option<Type>,
    recover_block_errors: bool,
    recovery_diagnostics: Vec<Diagnostic>,
}

impl<'a> Analyzer<'a> {
    fn new(
        signatures: &'a HashMap<String, FunctionSignature>,
        structs: &'a [RynStruct],
        struct_ids: &'a HashMap<String, usize>,
    ) -> Self {
        Self {
            names: HashMap::new(),
            local_types: Vec::new(),
            signatures,
            structs,
            struct_ids,
            loop_depth: 0,
            function_name: String::new(),
            return_type: None,
            recover_block_errors: false,
            recovery_diagnostics: Vec::new(),
        }
    }

    fn function(mut self, mut function: Function) -> Result<RynFunction, Diagnostic> {
        let (parameters, body_always_returns) = self.prepare_function(&function)?;
        let statements = self.block(std::mem::take(&mut function.body))?;
        self.finish_function(function, parameters, body_always_returns, statements)
    }

    fn function_recovering(
        mut self,
        mut function: Function,
    ) -> Result<RynFunction, Vec<Diagnostic>> {
        let (parameters, body_always_returns) = self
            .prepare_function(&function)
            .map_err(|error| vec![error])?;
        self.recover_block_errors = true;
        let (statements, top_level_declaration_failed) =
            self.recover_block(std::mem::take(&mut function.body), true);
        let mut diagnostics = std::mem::take(&mut self.recovery_diagnostics);
        if top_level_declaration_failed {
            diagnostics.sort_by_key(|diagnostic| diagnostic.span.start);
            return Err(diagnostics);
        }
        let finished = self.finish_function(function, parameters, body_always_returns, statements);
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
        self.return_type = self.signatures[&function.name].return_type;
        let body_always_returns = block_always_returns(&function.body);
        let mut parameters = Vec::with_capacity(function.parameters.len());
        for (parameter, ty) in function
            .parameters
            .iter()
            .zip(&self.signatures[&function.name].parameters)
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
        Ok((parameters, body_always_returns))
    }

    fn finish_function(
        &mut self,
        function: Function,
        parameters: Vec<LocalBinding>,
        body_always_returns: bool,
        statements: Vec<IrStatement>,
    ) -> Result<RynFunction, Diagnostic> {
        let return_type = self.return_type;
        let return_value = match (return_type, function.return_value) {
            (Some(expected), Some(value)) => {
                let span = value.span();
                let (value, actual) = self.expression(value, Some(expected))?;
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
        Ok(RynFunction {
            parameters,
            return_type,
            return_value,
            statements,
            local_types: std::mem::take(&mut self.local_types),
        })
    }

    fn allocate(&mut self, ty: Type) -> usize {
        let slot = self.local_types.len();
        self.allocate_storage(ty);
        slot
    }

    fn allocate_storage(&mut self, ty: Type) {
        if let Type::Struct(struct_id) = ty {
            let field_types = self.structs[struct_id]
                .fields
                .iter()
                .map(|field| field.ty)
                .collect::<Vec<_>>();
            for field_ty in field_types {
                self.allocate_storage(field_ty);
            }
        } else {
            self.allocate_scalar(ty);
        }
    }

    fn allocate_scalar(&mut self, ty: Type) {
        match ty {
            Type::I8 | Type::U8 => self.local_types.push(LocalType::I8),
            Type::I16 | Type::U16 => self.local_types.push(LocalType::I16),
            Type::I32 => self.local_types.push(LocalType::I32),
            Type::I64 => self.local_types.push(LocalType::I64),
            Type::U32 => self.local_types.push(LocalType::I32),
            Type::U64 => self.local_types.push(LocalType::I64),
            Type::F32 => self.local_types.push(LocalType::F32),
            Type::F64 => self.local_types.push(LocalType::F64),
            Type::Str => {
                self.local_types.push(LocalType::Ptr);
                self.local_types.push(LocalType::I64);
            }
            Type::Bool => self.local_types.push(LocalType::I8),
            Type::Struct(_) => unreachable!("structure storage is allocated field by field"),
        }
    }

    fn block(&mut self, body: Vec<Statement>) -> Result<Vec<IrStatement>, Diagnostic> {
        if !self.recover_block_errors {
            return body.into_iter().map(|stmt| self.statement(stmt)).collect();
        }

        Ok(self.recover_block(body, false).0)
    }

    fn recover_block(
        &mut self,
        body: Vec<Statement>,
        function_body: bool,
    ) -> (Vec<IrStatement>, bool) {
        let mut statements = Vec::with_capacity(body.len());
        let mut stopped_on_declaration = false;
        for statement in body {
            let names_before = self.names.clone();
            let local_type_count = self.local_types.len();
            let loop_depth = self.loop_depth;
            let failed_declaration_blocks_recovery = matches!(
                &statement,
                Statement::Let { name, .. } if !self.names.contains_key(name)
            );
            match self.statement(statement) {
                Ok(statement) => statements.push(statement),
                Err(diagnostic) => {
                    self.names = names_before;
                    self.local_types.truncate(local_type_count);
                    self.loop_depth = loop_depth;
                    self.recovery_diagnostics.push(diagnostic);
                    if failed_declaration_blocks_recovery {
                        stopped_on_declaration = true;
                        break;
                    }
                }
            }
        }
        (statements, function_body && stopped_on_declaration)
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
        self.loop_depth += 1;
        let _ = self.block(body);
        self.loop_depth -= 1;
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
            .map(|name| resolve_type_name(name, self.struct_ids))
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
        match statement {
            Statement::Let {
                name,
                mutable,
                annotation,
                value,
                span,
            } => {
                if self.names.contains_key(&name) {
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
                    .map(|name| resolve_type_name(name, self.struct_ids))
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
                self.names.insert(name, Binding { slot, ty, mutable });
                Ok(IrStatement::Let { slot, ty, value })
            }
            Statement::Assign { name, value, span } => {
                let Some(binding) = self.names.get(&name).copied() else {
                    let error = self.unknown_variable(&name, span);
                    self.check_discarded_expression(value);
                    return Err(error);
                };
                if !binding.mutable {
                    let error =
                        diag("R0204", format!("`{name}` is immutable"), span).with_help(format!(
                            "declare `{name}` with `let mut` if you intend to assign a new value"
                        ));
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
                if !binding.mutable {
                    let error = diag("R0204", format!("`{object}` is immutable"), object_span)
                        .with_help(format!("declare `{object}` with `let mut` if you intend to change one of its fields"));
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
                            "declare `{name}` with `let mut` if you intend to assign a new value"
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
                })
            }
            Statement::Print(expr, span) => {
                let (value, ty) = self.expression(expr, None).map_err(|mut e| {
                    if e.span == Span::default() {
                        e.span = span;
                    }
                    e
                })?;
                Ok(IrStatement::Print { value, ty })
            }
            Statement::PrintTemplate(parts, span) => {
                let mut lowered = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        PrintPart::Text(value) => lowered.push(IrPrintPart::Text(value)),
                        PrintPart::Value(expression) => match self.expression(expression, None) {
                            Ok((value, ty)) => {
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
            } => {
                let (target, arguments, _) = self.lower_call(name, arguments, span)?;
                Ok(IrStatement::Call { target, arguments })
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
                        "`if` condition must have type `bool`",
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
                let outer_names = self.names.clone();
                let then_body = self.block(then_body)?;
                self.names = outer_names.clone();
                let else_body = self.block(else_body)?;
                self.names = outer_names;
                Ok(IrStatement::If {
                    condition,
                    then_body,
                    else_body,
                })
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
                        let outer_names = self.names.clone();
                        self.loop_depth += 1;
                        let _ = self.block(body);
                        self.loop_depth -= 1;
                        self.names = outer_names;
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                if ty != Type::Bool {
                    let error = diag(
                        "R0207",
                        "`while` condition must have type `bool`",
                        condition_span,
                    )
                    .with_help("use a boolean expression, such as a comparison, for the condition");
                    if self.recover_block_errors {
                        let outer_names = self.names.clone();
                        self.loop_depth += 1;
                        let _ = self.block(body);
                        self.loop_depth -= 1;
                        self.names = outer_names;
                    }
                    return Err(error);
                }
                let outer_names = self.names.clone();
                self.loop_depth += 1;
                let body = self.block(body);
                self.loop_depth -= 1;
                let body = body?;
                self.names = outer_names;
                Ok(IrStatement::While { condition, body })
            }
            Statement::For {
                name,
                name_span,
                start,
                end,
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

                let (start, ty) = match self.expression(start, None) {
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
                self.loop_depth += 1;
                let body = self.block(body);
                self.loop_depth -= 1;
                self.names = outer_names;
                let body = body?;
                Ok(IrStatement::For {
                    slot,
                    end_slot,
                    ty,
                    start,
                    end,
                    body,
                })
            }
            Statement::Break(span) => {
                if self.loop_depth == 0 {
                    return Err(
                        diag("R0016", "`break` can only be used inside a loop", span)
                            .with_help("move `break` into the body of a `while` or `for` loop"),
                    );
                }
                Ok(IrStatement::Break)
            }
            Statement::Continue(span) => {
                if self.loop_depth == 0 {
                    return Err(
                        diag("R0017", "`continue` can only be used inside a loop", span)
                            .with_help("move `continue` into the body of a `while` or `for` loop"),
                    );
                }
                Ok(IrStatement::Continue)
            }
            Statement::Return { value, span } => {
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
                Ok(IrStatement::Return { value })
            }
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
            Expression::String(s, _) => Ok((IrExpression::String(s), Type::Str)),
            Expression::Boolean(value, _) => Ok((IrExpression::Boolean(value), Type::Bool)),
            Expression::Name(name, span) => {
                let binding = self
                    .names
                    .get(&name)
                    .ok_or_else(|| self.unknown_variable(&name, span))?;
                Ok((
                    IrExpression::Local {
                        slot: binding.slot,
                        ty: binding.ty,
                    },
                    binding.ty,
                ))
            }
            Expression::StructLiteral { name, fields, span } => {
                if self.recover_block_errors {
                    return self.struct_literal_recovering(name, fields, span, expected);
                }
                let struct_id = *self.struct_ids.get(&name).ok_or_else(|| {
                    diag("R0227", format!("unknown structure `{name}`"), span)
                        .with_help("declare the structure before constructing it")
                })?;
                let ty = Type::Struct(struct_id);
                if expected.is_some_and(|expected| expected != ty) {
                    return Err(diag(
                        "R0205",
                        format!(
                            "structure literal `{name}` does not match the expected type `{}`",
                            expected.map(type_name).unwrap_or("unknown")
                        ),
                        span,
                    ));
                }
                let definition = &self.structs[struct_id];
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
                let Type::Struct(struct_id) = actual else {
                    return Err(
                        diag("R0224", "field access requires a structure value", span)
                            .with_help("access a field on a value declared with a structure type"),
                    );
                };
                let (field_index, field_info) = self.structs[struct_id]
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.name == name)
                    .ok_or_else(|| self.unknown_struct_field(struct_id, &name, name_span))?;
                Ok((
                    IrExpression::Field {
                        value: Box::new(value),
                        struct_id,
                        field_index,
                        ty: field_info.ty,
                    },
                    field_info.ty,
                ))
            }
            Expression::Call {
                name,
                arguments,
                span,
            } => {
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
                        "`if` expression condition must have type `bool`",
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
                            "if-expression branches must have the same type, found `{}` and `{}`",
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
                let target = resolve_type_name(&target, self.struct_ids)?;
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
                if !is_numeric(source) {
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
                let (right, right_ty) =
                    self.expression(*right, if shift { Some(Type::U32) } else { context })?;
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
                            is_numeric(left_ty) && left_ty == right_ty
                        };
                        if !valid {
                            let bad_operand_span = if equality {
                                if !is_equality_type(left_ty) {
                                    left_span
                                } else {
                                    right_span
                                }
                            } else if !is_numeric(left_ty) {
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
        let Some(struct_id) = self.struct_ids.get(&name).copied() else {
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
        let definition = &self.structs[struct_id];
        let mut diagnostics = Vec::new();
        if expected.is_some_and(|expected| expected != ty) {
            diagnostics.push(diag(
                "R0205",
                format!(
                    "structure literal `{name}` does not match the expected type `{}`",
                    expected.map(type_name).unwrap_or("unknown")
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
            Expression::Name(name, _) => self
                .names
                .get(name)
                .map(|binding| binding.ty)
                .filter(|ty| is_numeric(*ty)),
            Expression::Call { name, .. } => self
                .signatures
                .get(name)
                .and_then(|signature| signature.return_type)
                .filter(|ty| is_numeric(*ty)),
            Expression::Field { value, name, .. } => {
                let Type::Struct(struct_id) = self.value_type_hint(value)? else {
                    return None;
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
            Expression::Cast(_, target, _) => resolve_type_name(target, self.struct_ids)
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
            Expression::Name(name, _) => self.names.get(name).map(|binding| binding.ty),
            Expression::StructLiteral { name, .. } => {
                self.struct_ids.get(name).copied().map(Type::Struct)
            }
            Expression::Call { name, .. } => self
                .signatures
                .get(name)
                .and_then(|signature| signature.return_type),
            Expression::Cast(_, target, _) => resolve_type_name(target, self.struct_ids).ok(),
            Expression::Field { value, name, .. } => {
                let Type::Struct(struct_id) = self.value_type_hint(value)? else {
                    return None;
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

    fn lower_call(
        &mut self,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
    ) -> Result<(IrCallTarget, Vec<IrExpression>, Option<Type>), Diagnostic> {
        let builtin = match name.as_str() {
            "arg_count" => Some((IrCallTarget::ArgumentCount, Vec::new(), Some(Type::U32))),
            "arg" => Some((IrCallTarget::Argument, vec![Type::U32], Some(Type::Str))),
            _ => None,
        };
        let Some(sig) = self.signatures.get(&name).cloned() else {
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
            match self.expression(argument, Some(*expected)) {
                Ok((argument, actual)) if actual == *expected => lowered.push(argument),
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
            match self.expression(argument, Some(expected)) {
                Ok((value, actual)) if actual == expected => lowered.push(value),
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
        TypeName::Bool => Type::Bool,
        TypeName::Named(name, span) => Type::Struct(*structs.get(name).ok_or_else(|| {
            diag("R0230", format!("unknown type `{name}`"), *span)
                .with_help("use a built-in type or declare this structure")
        })?),
    })
}

fn collect_struct_declaration_errors(
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
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
            if let Err(diagnostic) = resolve_type_name(&field.ty, struct_ids) {
                diagnostics.push(diagnostic);
            }
        }
    }
    diagnostics
}

fn collect_function_signature_errors(
    functions: &[Function],
    struct_ids: &HashMap<String, usize>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for function in functions {
        for parameter in &function.parameters {
            if let Err(diagnostic) = resolve_type_name(&parameter.ty, struct_ids) {
                diagnostics.push(diagnostic);
            }
        }
        if let Some(return_type) = &function.return_type
            && let Err(diagnostic) = resolve_type_name(return_type, struct_ids)
        {
            diagnostics.push(diagnostic);
        }
        if function.name == "main"
            && (!function.parameters.is_empty()
                || function
                    .return_type
                    .as_ref()
                    .and_then(|name| resolve_type_name(name, struct_ids).ok())
                    .is_some_and(|return_type| return_type != Type::I32))
        {
            diagnostics.push(
                diag(
                    "R0208",
                    "`main` must take no parameters and return either no value or `i32`",
                    function.span,
                )
                .with_help("use `fn main() { ... }` or `fn main() -> i32 { ... }`; move reusable work into another function"),
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

fn resolve_struct_layouts(
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
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
            let Some(&child_id) = struct_ids.get(name) else {
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

fn resolve_struct_layout(
    struct_id: usize,
    via_field: Option<Span>,
    depth: usize,
    definitions: &[StructDef],
    struct_ids: &HashMap<String, usize>,
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
        let ty = resolve_type_name(&field.ty, struct_ids)?;
        let width = if let Type::Struct(child_id) = ty {
            resolve_struct_layout(
                child_id,
                Some(field.span),
                depth + 1,
                definitions,
                struct_ids,
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
                        field.span,
                    )
                })?
        } else {
            slot_width(ty)
        };
        fields.push(RynStructField {
            name: field.name.clone(),
            ty,
            slot_offset: slot_count,
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
        name: definition.name.clone(),
        fields,
        slot_count,
    });
    states[struct_id] = 2;
    Ok(())
}

fn slot_width(ty: Type) -> usize {
    if ty == Type::Str { 2 } else { 1 }
}
fn type_name(ty: Type) -> &'static str {
    match ty {
        Type::I8 => "i8",
        Type::I16 => "i16",
        Type::I32 => "i32",
        Type::I64 => "i64",
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Str => "str",
        Type::Bool => "bool",
        Type::Struct(_) => "structure",
    }
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
    is_numeric(ty) || matches!(ty, Type::Str | Type::Bool | Type::Struct(_))
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
    fn process_argument_builtins_have_checked_signatures_and_can_be_shadowed() {
        let wrong_count = error("fn main() { arg_count(0) }");
        assert_eq!(wrong_count.code, "R0211");

        let wrong_index_type = error("fn main() { arg(\"0\") }");
        assert_eq!(wrong_index_type.code, "R0212");

        analyze(
            parse("fn arg_count() -> i32 { 9 } fn main() { print(arg_count()) }")
                .expect("shadowing source parses"),
        )
        .expect("a user function may shadow the builtin name");
    }

    #[test]
    fn rejects_assignment_to_immutable_local() {
        let diagnostic = error("fn main() { let value = 1 value = 2 }");
        assert_eq!(diagnostic.code, "R0204");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("declare `value` with `let mut` if you intend to assign a new value")
        );

        let compound = error("fn main() { let value = 1 value += 2 }");
        assert_eq!(compound.code, "R0204");
        assert_eq!(compound.message, "`value` is immutable");
    }

    #[test]
    fn structural_diagnostics_suggest_direct_repairs() {
        let missing_main = analyze(parse("fn helper() {}").expect("source parses"))
            .expect_err("program without main should fail");
        assert_eq!(missing_main.code, "R0200");
        assert_eq!(
            missing_main.help.as_deref(),
            Some("add a top-level `fn main() { ... }` function")
        );

        let duplicate_function = error("fn main() {} fn main() {}");
        assert_eq!(duplicate_function.code, "R0201");
        assert_eq!(
            duplicate_function.help.as_deref(),
            Some("give each function a unique name; function overloading is not supported")
        );

        let duplicate_parameter = error("fn take(value: i32, value: i32) {} fn main() {}");
        assert_eq!(duplicate_parameter.code, "R0202");
        assert_eq!(
            duplicate_parameter.help.as_deref(),
            Some("choose a unique name for each parameter in the function")
        );

        let duplicate_local = error("fn main() { let value = 1 let value = 2 }");
        assert_eq!(duplicate_local.code, "R0202");
        assert_eq!(
            duplicate_local.help.as_deref(),
            Some("choose a different name or remove the second declaration of `value`")
        );

        let invalid_main = error("fn main(value: i32) {}");
        assert_eq!(invalid_main.code, "R0208");
        assert_eq!(
            invalid_main.help.as_deref(),
            Some(
                "use `fn main() { ... }` or `fn main() -> i32 { ... }`; move reusable work into another function"
            )
        );

        let unsupported_main_result = error("fn main() -> i64 { 0 }");
        assert_eq!(unsupported_main_result.code, "R0208");
    }
    #[test]
    fn rejects_arithmetic_on_strings() {
        assert_eq!(
            error("fn main() { let value = \"x\" + \"y\" }").code,
            "R0206"
        );
    }

    #[test]
    fn bitwise_operators_require_matching_integer_types() {
        analyze(
            parse(
                "fn main() { let left: u8 = 0b1010 let right: u8 = 0b1100 let both = left & right let either = left | right let different = left ^ right let inverted = ~left print(both) print(either) print(different) print(inverted) }",
            )
            .expect("integer bitwise source parses"),
        )
        .expect("matching integer bitwise operands type-check");

        let mismatched =
            error("fn main() { let left: i32 = 1 let right: u32 = 2 let value = left & right }");
        assert_eq!(mismatched.code, "R0206");
        assert_eq!(
            mismatched.message,
            "bitwise operands must have matching integer types, found `i32` and `u32`"
        );
        assert_eq!(
            &"fn main() { let left: i32 = 1 let right: u32 = 2 let value = left & right }"
                [mismatched.span.start..mismatched.span.end],
            "right"
        );

        assert_eq!(error("fn main() { let value = 1.0 & 2.0 }").code, "R0206");
        assert_eq!(error("fn main() { let value = ~true }").code, "R0206");
    }

    #[test]
    fn if_expression_requires_a_boolean_condition_and_matching_branch_types() {
        analyze(
            parse("fn main() { let result: i32 = if true { 1 } else { 2 } }")
                .expect("source parses"),
        )
        .expect("matching if-expression branches type-check");

        let condition = error("fn main() { let result = if 1 { 1 } else { 2 } }");
        assert_eq!(condition.code, "R0207");
        assert_eq!(
            condition.message,
            "`if` expression condition must have type `bool`"
        );

        let source = "fn main() { let result = if true { 1 } else { 2.0 } }";
        let branches = error(source);
        let else_value = source.rfind("2.0").expect("else value is present");
        assert_eq!(branches.code, "R0205");
        assert_eq!(branches.span.start, else_value);
        assert_eq!(branches.span.end, else_value + "2.0".len());
        assert_eq!(
            branches.message,
            "if-expression branches must have the same type, found `i64` and `f64`"
        );
    }

    #[test]
    fn if_statements_with_void_calls_remain_statements_before_a_tail_value() {
        let source = "fn notify() { print(1) } fn answer(flag: bool) -> i32 { if flag { notify() } else { notify() } 42 } fn main() { print(answer(false)) }";
        analyze(parse(source).expect("source parses"))
            .expect("statement branches do not become void-valued if expressions");
    }

    #[test]
    fn checks_i32_literal_range_and_mixed_integer_types() {
        assert_eq!(
            error("fn main() { let value: i32 = 2147483648 }").code,
            "R0206"
        );
        assert_eq!(
            error("fn main() { let value: i32 = 1 let wide: i64 = 2 print(value + wide) }").code,
            "R0206"
        );
    }

    #[test]
    fn compound_assignment_requires_compatible_numeric_types() {
        let diagnostic =
            error("fn main() { let mut narrow: i32 = 1 let wide: i64 = 2 narrow += wide }");
        assert_eq!(diagnostic.code, "R0206");
        assert_eq!(
            diagnostic.message,
            "arithmetic operands must have matching numeric types, found `i32` and `i64`"
        );
        assert_eq!(
            diagnostic.span.start,
            "fn main() { let mut narrow: i32 = 1 let wide: i64 = 2 narrow += wide }"
                .rfind("wide")
                .expect("right operand is present")
        );
    }

    #[test]
    fn bitwise_compound_assignment_requires_matching_integer_types() {
        let float = error("fn main() { let mut value: f32 = 1.0 value &= 1.0 }");
        assert_eq!(float.code, "R0206");
        assert!(float.message.contains("bitwise operands"));

        let mixed = error("fn main() { let mut narrow: i32 = 1 let wide: i64 = 2 narrow |= wide }");
        assert_eq!(mixed.code, "R0206");
        assert!(
            mixed
                .message
                .contains("bitwise operands must have matching integer types")
        );
    }

    #[test]
    fn shifts_require_an_integer_value_and_u32_count() {
        let mixed_count = error("fn main() { let amount: i32 = 2 print(1 << amount) }");
        assert_eq!(mixed_count.code, "R0206");
        assert!(mixed_count.message.contains("`u32` count"));

        analyze(
            parse("fn main() { let amount: u32 = 2 let value: i8 = 1 << amount print(value) }")
                .expect("typed shift source parses"),
        )
        .expect("the left value keeps its type and the shift count is u32");
    }

    #[test]
    fn numeric_casts_accept_numeric_sources_and_targets() {
        analyze(
            parse("fn main() { let signed: i8 = -1 let widened: u16 = signed as u16 let narrowed: i8 = widened as i8 let maximum: u64 = 18446744073709551615 let reinterpret: i64 = maximum as i64 let small: i32 = 16777217 let rounded: f32 = small as f32 let promoted: f64 = rounded as f64 let fractional: f64 = 12.9 let integer: i32 = fractional as i32 }")
                .expect("numeric cast source parses"),
        )
        .expect("numeric casts support integer and floating-point conversions");

        let non_numeric_source = error("fn main() { let value = true as i32 }");
        assert_eq!(non_numeric_source.code, "R0206");
        assert!(
            non_numeric_source
                .message
                .contains("numeric cast requires a numeric value")
        );

        let non_numeric_target = error("fn main() { let value = 1 as bool }");
        assert_eq!(non_numeric_target.code, "R0206");
        assert!(
            non_numeric_target
                .message
                .contains("numeric cast requires a numeric target")
        );
    }

    #[test]
    fn operator_type_diagnostics_highlight_the_incompatible_operand() {
        let arithmetic_source =
            "fn main() { let left: i32 = 1 let right: i64 = 2 print(left + right) }";
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

        let comparison_source =
            "fn main() { let left: i32 = 1 let right: i64 = 2 print(left < right) }";
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

        let logical_source =
            "fn main() { let left: bool = true let right: i32 = 1 print(left && right) }";
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
            "fn main() { let text: str = \"x\" let number: i32 = 1 print(text + number) }";
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
            parse("fn main() { print(true == false) print(\"a\" != \"b\") }")
                .expect("source parses"),
        )
        .expect("bool and string equality are supported");

        let mixed = error("fn main() { print(true == 1) }");
        assert_eq!(mixed.code, "R0206");
        assert_eq!(
            mixed.message,
            "equality operands are incompatible: found `bool` and `i64`"
        );
        assert_eq!(
            mixed.help.as_deref(),
            Some("compare values with the same type")
        );

        let ordered = error("fn main() { print(true < false) }");
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
            parse("fn main() { let min: i8 = -128 let max: u64 = 18446744073709551615 print(min) print(max) }")
                .expect("source parses"),
        )
        .expect("representable boundary values are accepted");
        let too_large_i8 = error("fn main() { let value: i8 = 128 }");
        assert_eq!(too_large_i8.code, "R0206");
        assert_eq!(
            too_large_i8.help.as_deref(),
            Some("use a value from `-128` to `127`, or choose a wider integer type")
        );
        assert_eq!(error("fn main() { let value: u8 = 256 }").code, "R0206");
        assert_eq!(
            error("fn main() { let value: i64 = 9223372036854775808 }").code,
            "R0206"
        );
        let unsigned_negation = error("fn main() { let value: u8 = 1 print(-value) }");
        assert_eq!(unsigned_negation.code, "R0206");
        assert_eq!(
            unsigned_negation.help.as_deref(),
            Some(
                "declare the value with a signed integer or floating-point type before negating it"
            )
        );
        let invalid_not = error("fn main() { print(!1) }");
        assert_eq!(invalid_not.code, "R0206");
        assert_eq!(
            invalid_not.help.as_deref(),
            Some("use `!` with a boolean value or comparison")
        );
    }

    #[test]
    fn accepts_i32_minimum_literal() {
        analyze(parse("fn main() { let value: i32 = -2147483648 print(value) }").expect("parses"))
            .expect("i32 minimum is representable");
    }

    #[test]
    fn rejects_out_of_range_f32_and_mixed_float_types() {
        let out_of_range = error("fn main() { let value: f32 = 3.5e38 }");
        assert_eq!(out_of_range.code, "R0206");
        assert_eq!(
            out_of_range.help.as_deref(),
            Some("use a smaller finite value or declare it as `f64`")
        );
        assert_eq!(
            error("fn main() { let narrow: f32 = 1.0 let wide: f64 = 2.0 print(narrow + wide) }")
                .code,
            "R0206"
        );
        assert_eq!(
            error(
                "fn main() { let integer: i32 = 1 let decimal: f32 = 2.0 print(integer + decimal) }"
            )
            .code,
            "R0206"
        );
        let modulo_source = "fn main() { print(1.0 % 0.5) }";
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
        assert_eq!(error("fn main() { print(missing) }").code, "R0203");
    }

    #[test]
    fn suggests_a_close_variable_name_only_when_the_match_is_unambiguous() {
        let diagnostic = error("fn main() { let count = 1 print(cout) }");
        assert_eq!(diagnostic.code, "R0203");
        assert_eq!(diagnostic.help.as_deref(), Some("did you mean `count`?"));

        let ambiguous = error("fn main() { let count = 1 let coat = 2 print(cout) }");
        assert_eq!(ambiguous.code, "R0203");
        assert!(ambiguous.help.is_none());

        let assignment = error("fn main() { let mut count = 1 cout = 2 }");
        assert_eq!(assignment.code, "R0203");
        assert_eq!(assignment.help.as_deref(), Some("did you mean `count`?"));

        let compound_assignment = error("fn main() { let mut count = 1 cout += 2 }");
        assert_eq!(compound_assignment.code, "R0203");
        assert_eq!(
            compound_assignment.help.as_deref(),
            Some("did you mean `count`?")
        );
    }

    #[test]
    fn suggests_a_close_function_name() {
        let diagnostic =
            error("fn calculate(value: i64) -> i64 { value } fn main() { print(calculat(1)) }");
        assert_eq!(diagnostic.code, "R0210");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("did you mean `calculate`?")
        );

        let builtin = error("fn main() { argg(0) }");
        assert_eq!(builtin.code, "R0210");
        assert_eq!(builtin.help.as_deref(), Some("did you mean `arg`?"));

        let builtin_with_underscore = error("fn main() { arg_coun(0) }");
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
        let diagnostic = error("fn main() { let value: i64 = \"wrong\" }");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("make the initializer a `i64` value or change the type annotation")
        );
    }

    #[test]
    fn type_mismatch_diagnostics_highlight_the_offending_value() {
        let initializer_source = "fn main() { let value: i32 = \"wrong\" }";
        let initializer = error(initializer_source);
        let initializer_start = initializer_source
            .find("\"wrong\"")
            .expect("initializer is present");
        assert_eq!(initializer.code, "R0205");
        assert_eq!(initializer.span.start, initializer_start);
        assert_eq!(initializer.span.end, initializer_start + "\"wrong\"".len());

        let assignment_source = "fn main() { let mut value: i32 = 1 value = \"wrong\" }";
        let assignment = error(assignment_source);
        let assignment_start = assignment_source
            .find("\"wrong\"")
            .expect("assigned value is present");
        assert_eq!(assignment.code, "R0205");
        assert_eq!(assignment.span.start, assignment_start);
        assert_eq!(assignment.span.end, assignment_start + "\"wrong\"".len());

        let argument_source = "fn take(value: i32) {} fn main() { take(\"wrong\") }";
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
        let source = r#"fn main() { print("λ\n{missing}") }"#;
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
        let source = "fn main() { if 1 { print(1) } }";
        let diagnostic = error(source);
        assert_eq!(diagnostic.code, "R0207");
        let condition_start = source.find("if 1").expect("if condition is present") + 3;
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
            error("fn main() { if true { let hidden = 1 } print(hidden) }").code,
            "R0203"
        );
    }
    #[test]
    fn requires_boolean_loop_conditions() {
        let source = "fn main() { while 1 { print(1) } }";
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
        let break_diagnostic = error("fn main() { break }");
        assert_eq!(break_diagnostic.code, "R0016");
        assert_eq!(
            break_diagnostic.message,
            "`break` can only be used inside a loop"
        );

        let continue_diagnostic = error("fn main() { continue }");
        assert_eq!(continue_diagnostic.code, "R0017");
        assert_eq!(
            continue_diagnostic.message,
            "`continue` can only be used inside a loop"
        );
    }
    #[test]
    fn validates_function_call_arguments() {
        let diagnostic =
            error("fn add(value: i64) -> i64 { value } fn main() { print(add(\"wrong\")) }");
        assert_eq!(diagnostic.code, "R0212");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass a `i64` value to `add`")
        );
    }
    #[test]
    fn checks_function_arity() {
        let diagnostic =
            error("fn add(a: i64, b: i64) -> i64 { a + b } fn main() { print(add(1)) }");
        assert_eq!(diagnostic.code, "R0211");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("pass exactly 2 arguments to `add`")
        );
    }
    #[test]
    fn rejects_unknown_function_calls() {
        assert_eq!(error("fn main() { print(missing()) }").code, "R0210");
    }
    #[test]
    fn checks_function_return_type() {
        let diagnostic = error("fn answer() -> i64 { \"wrong\" } fn main() { print(1) }");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i64` value or change the function's declared result type")
        );
    }

    #[test]
    fn checks_explicit_return_statement_types() {
        let diagnostic = error("fn answer() -> i32 { return \"wrong\" } fn main() {}");
        assert_eq!(diagnostic.code, "R0205");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i32` value or change the function's declared result type")
        );
    }

    #[test]
    fn requires_a_result_type_for_explicit_return_statements() {
        let diagnostic = error("fn answer() { return 42 } fn main() {}");
        assert_eq!(diagnostic.code, "R0013");
        assert!(diagnostic.message.contains("must declare its type"));
    }

    #[test]
    fn accepts_bare_return_in_void_functions() {
        let source = "fn notify(enabled: bool) { if !enabled { return } print(1) } fn main() { notify(false) }";
        assert!(analyze(parse(source).expect("source parses")).is_ok());
    }

    #[test]
    fn requires_a_value_when_returning_from_a_value_function() {
        let diagnostic = error("fn answer() -> i32 { return } fn main() {}");
        assert_eq!(diagnostic.code, "R0216");
        assert_eq!(diagnostic.help.as_deref(), Some("return a `i32` value"));
    }

    #[test]
    fn accepts_functions_that_return_from_every_if_else_branch() {
        let source =
            "fn answer(flag: bool) -> i32 { if flag { return 1 } else { return 2 } } fn main() {}";
        assert!(analyze(parse(source).expect("source parses")).is_ok());

        let partial = error("fn answer(flag: bool) -> i32 { if flag { return 1 } } fn main() {}");
        assert_eq!(partial.code, "R0213");
    }

    #[test]
    fn suggests_returning_a_value_from_non_void_functions() {
        let diagnostic = error("fn answer() -> i64 {} fn main() {}");
        assert_eq!(diagnostic.code, "R0213");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("return a `i64` value from this function")
        );
    }

    #[test]
    fn suggests_a_result_type_for_tail_expressions() {
        let diagnostic = error("fn answer() { 42 } fn main() {}");
        assert_eq!(diagnostic.code, "R0214");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("add a `-> TYPE` result type to `answer` or remove its result expression")
        );
    }

    #[test]
    fn permits_void_calls_as_statements_but_not_values() {
        let source = "fn notify() { print(\"hi\") } fn main() { notify() }";
        assert!(analyze(parse(source).expect("parses")).is_ok());
        let diagnostic = error("fn notify() { print(\"hi\") } fn main() { print(notify()) }");
        assert_eq!(diagnostic.code, "R0215");
        assert_eq!(
            diagnostic.help.as_deref(),
            Some("call `notify` as a statement or give it a return type and value")
        );
    }

    #[test]
    fn structures_validate_declarations_literals_and_field_mutability() {
        let valid = "struct Pair { left: i32, right: str } struct Boxed { pair: Pair, enabled: bool } fn identity(value: Boxed) -> Boxed { return value } fn main() { let mut boxed = Boxed { enabled: true, pair: Pair { right: \"r\", left: 3 } } boxed.pair.left = 4 print(identity(boxed).pair.left) }";
        analyze(parse(valid).expect("valid structure program parses"))
            .expect("nested fields and by-value function signatures type-check");

        assert_eq!(error("struct Empty {} fn main() {}").code, "R0221");
        assert_eq!(
            error("struct A { x: i32, x: bool } fn main() {}").code,
            "R0222"
        );
        assert_eq!(error("struct A { x: Missing } fn main() {}").code, "R0230");
        assert_eq!(error("struct A { a: A } fn main() {}").code, "R0232");
        assert_eq!(
            error("struct A { b: B } struct B { a: A } fn main() {}").code,
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
        deeply_nested.push_str(" fn main() {}");
        assert_eq!(error(&deeply_nested).code, "R0233");
        assert_eq!(
            error("struct A { x: i32, y: i32 } fn main() { let a = A { x: 1 } }").code,
            "R0229"
        );
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { x: 1, x: 2 } }").code,
            "R0228"
        );
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { y: 1 } }").code,
            "R0225"
        );
        let literal_field_typo =
            error("struct A { width: i32 } fn main() { let a = A { widt: 1 } }");
        assert_eq!(literal_field_typo.code, "R0225");
        assert_eq!(
            literal_field_typo.help.as_deref(),
            Some("did you mean `width`?")
        );
        let access_field_typo =
            error("struct A { width: i32 } fn main() { let a = A { width: 1 } print(a.widt) }");
        assert_eq!(access_field_typo.code, "R0225");
        assert_eq!(
            access_field_typo.help.as_deref(),
            Some("did you mean `width`?")
        );
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { x: true } }").code,
            "R0205"
        );
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { x: 1 } a.x = 2 }").code,
            "R0204"
        );
        analyze(
            parse("struct A { x: i32 } fn main() { let a = A { x: 1 } print(a) print(\"a={a}\") }")
                .expect("structure printing source parses"),
        )
        .expect("structures may be printed directly or interpolated");
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { x: 1 } print(a.missing) }").code,
            "R0225"
        );
        assert_eq!(
            error("fn main() { let value = 1 print(value.field) }").code,
            "R0224"
        );
        assert_eq!(
            error("struct A { x: i32 } fn main() { let mut a = A { x: 1 } a.x.y = 2 }").code,
            "R0224"
        );
        assert_eq!(
            error(
                "struct A { text: str } fn main() { let mut a = A { text: \"x\" } a.text += \"y\" }"
            )
            .code,
            "R0206"
        );
        assert_eq!(
            error(
                "struct A { count: i32 } fn main() { let mut a = A { count: 1 } a.count += 0.5 }"
            )
            .code,
            "R0206"
        );
        let structure_equality = "struct A { x: i32 } fn main() { let a = A { x: 1 } let b = A { x: 1 } print(a == b) print(a != b) }";
        analyze(parse(structure_equality).expect("structure equality source parses"))
            .expect("matching structure values support equality");
        assert_eq!(
            error("struct A { x: i32 } fn main() { let a = A { x: 1 } let b = A { x: 2 } print(a < b) }").code,
            "R0206"
        );
        assert_eq!(
            error("struct A { x: i32 } struct B { x: i32 } fn main() { let a = A { x: 1 } let b = B { x: 1 } print(a == b) }").code,
            "R0206"
        );
    }
}
