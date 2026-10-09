//! Mutable traversal of the syntax tree, shared by the passes that rewrite it
//! before semantic analysis (pattern compilation, compile-time evaluation).

use crate::{
    ast::{Expression, PrintPart, Program, Statement},
    source::Diagnostic,
};

pub type Visitor<'a> = dyn FnMut(&mut Expression) -> Result<(), Diagnostic> + 'a;

/// Calls `visitor` on every expression in the program, children before parents.
pub fn visit_program(program: &mut Program, visitor: &mut Visitor<'_>) -> Result<(), Diagnostic> {
    for function in &mut program.functions {
        visit_statements(&mut function.body, visitor)?;
        if let Some(value) = &mut function.return_value {
            visit_expression(value, visitor)?;
        }
    }
    for shape in &mut program.shapes {
        for method in &mut shape.methods {
            if let Some(body) = &mut method.default_body {
                visit_statements(body, visitor)?;
            }
            if let Some(value) = &mut method.default_value {
                visit_expression(value, visitor)?;
            }
        }
    }
    Ok(())
}

pub fn visit_statements(
    statements: &mut [Statement],
    visitor: &mut Visitor<'_>,
) -> Result<(), Diagnostic> {
    for statement in statements {
        visit_statement(statement, visitor)?;
    }
    Ok(())
}

pub fn visit_statement(
    statement: &mut Statement,
    visitor: &mut Visitor<'_>,
) -> Result<(), Diagnostic> {
    match statement {
        Statement::Let { value, .. }
        | Statement::Assign { value, .. }
        | Statement::CompoundAssign { value, .. }
        | Statement::FieldAssign { value, .. }
        | Statement::Print(value, _) => visit_expression(value, visitor)?,
        Statement::IndexAssign { index, value, .. } => {
            visit_expression(index, visitor)?;
            visit_expression(value, visitor)?;
        }
        Statement::DereferenceAssign { pointer, value, .. } => {
            visit_expression(pointer, visitor)?;
            visit_expression(value, visitor)?;
        }
        Statement::PrintTemplate(parts, _) => {
            for part in parts {
                if let PrintPart::Value(value) = part {
                    visit_expression(value, visitor)?;
                }
            }
        }
        Statement::Call { arguments, .. } => {
            for argument in arguments {
                visit_expression(argument, visitor)?;
            }
        }
        Statement::MethodCall {
            value, arguments, ..
        } => {
            visit_expression(value, visitor)?;
            for argument in arguments {
                visit_expression(argument, visitor)?;
            }
        }
        Statement::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            visit_expression(condition, visitor)?;
            visit_statements(then_body, visitor)?;
            visit_statements(else_body, visitor)?;
        }
        Statement::While {
            condition, body, ..
        } => {
            visit_expression(condition, visitor)?;
            visit_statements(body, visitor)?;
        }
        Statement::Defer { body, .. } => visit_statements(body, visitor)?,
        Statement::For {
            start, end, body, ..
        } => {
            visit_expression(start, visitor)?;
            visit_expression(end, visitor)?;
            visit_statements(body, visitor)?;
        }
        Statement::ForEach {
            collection, body, ..
        } => {
            visit_expression(collection, visitor)?;
            visit_statements(body, visitor)?;
        }
        Statement::Return { value, .. } => {
            if let Some(value) = value {
                visit_expression(value, visitor)?;
            }
        }
        Statement::Break(_) | Statement::Continue(_) => {}
    }
    Ok(())
}

/// The direct sub-expressions of `expression`, in evaluation order.
pub fn children_mut(expression: &mut Expression) -> Vec<&mut Expression> {
    match expression {
        Expression::Integer(..)
        | Expression::Float(..)
        | Expression::String(..)
        | Expression::Character(..)
        | Expression::Boolean(..)
        | Expression::Name(..)
        | Expression::LayoutOf { .. }
        | Expression::VecConstructor { .. }
        | Expression::MapConstructor { .. }
        | Expression::SetConstructor { .. } => Vec::new(),
        Expression::Call { arguments, .. } | Expression::EnumConstruct { arguments, .. } => {
            arguments.iter_mut().collect()
        }
        Expression::MethodCall {
            value, arguments, ..
        } => std::iter::once(&mut **value)
            .chain(arguments.iter_mut())
            .collect(),
        Expression::StructLiteral { fields, .. } => {
            fields.iter_mut().map(|(_, value, _)| value).collect()
        }
        Expression::Field { value, .. }
        | Expression::Negate(value, _)
        | Expression::Not(value, _)
        | Expression::BitNot(value, _)
        | Expression::Dereference(value, _)
        | Expression::Cast(value, _, _)
        | Expression::Propagate(value, _)
        | Expression::AddressOf { value, .. }
        | Expression::ArrayRepeat { value, .. } => vec![&mut **value],
        Expression::Range { start, end, .. } => vec![&mut **start, &mut **end],
        Expression::If {
            condition,
            then_value,
            else_value,
            ..
        } => vec![&mut **condition, &mut **then_value, &mut **else_value],
        Expression::Binary { left, right, .. } => vec![&mut **left, &mut **right],
        Expression::Tuple(values, _) | Expression::ArrayLiteral(values, _) => {
            values.iter_mut().collect()
        }
        Expression::Index { value, index, .. } => vec![&mut **value, &mut **index],
        Expression::Choose { value, arms, .. } => std::iter::once(&mut **value)
            .chain(arms.iter_mut().map(|arm| &mut arm.body))
            .collect(),
    }
}

pub fn visit_expression(
    expression: &mut Expression,
    visitor: &mut Visitor<'_>,
) -> Result<(), Diagnostic> {
    for child in children_mut(expression) {
        visit_expression(child, visitor)?;
    }
    visitor(expression)
}
