//! Initial Ryn Guard ownership pass. Owning values move at assignments, returns
//! and by-value function arguments; observations and String methods borrow.
use crate::{
    sema::{IrCallTarget, IrExpression, IrPrintPart, IrStatement, RynIr, RynStruct, Type},
    source::{Diagnostic, Span},
};

pub(crate) fn owned_slots(ty: Type, slot: usize, structs: &[RynStruct]) -> Vec<(usize, Type)> {
    match ty {
        Type::OwnedString => vec![(slot, ty)],
        Type::Vec(_) => vec![(slot, ty)],
        Type::Map(_) => vec![(slot, ty)],
        Type::Set(_) => vec![(slot, ty)],
        Type::Enum(_) => vec![(slot, ty)],
        Type::Struct(id) if structs[id].drop_function.is_some() => vec![(slot, ty)],
        Type::Struct(id) => structs[id]
            .fields
            .iter()
            .flat_map(|field| owned_slots(field.ty, slot + field.slot_offset, structs))
            .collect(),
        Type::Array(id) => {
            let (element, length) = crate::sema::array_info(id);
            (0..length)
                .flat_map(|index| {
                    owned_slots(
                        element,
                        slot + index * crate::sema::storage_slot_width(element, structs),
                        structs,
                    )
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn check(ir: &mut RynIr) -> Result<(), Diagnostic> {
    let reference_return_parameters = ir
        .functions
        .iter()
        .map(|function| function.reference_return_parameter)
        .collect::<Vec<_>>();
    for (function_id, function) in ir.functions.iter_mut().enumerate() {
        let is_destructor = ir
            .structs
            .iter()
            .any(|structure| structure.drop_function == Some(function_id));
        let mut guard = Guard {
            structs: &ir.structs,
            reference_return_parameters: &reference_return_parameters,
            available: vec![false; function.local_types.len()],
            moved_at: vec![Span::default(); function.local_types.len()],
            borrowed: vec![0; function.local_types.len()],
            scopes: vec![Vec::new()],
            reference_scopes: vec![Vec::new()],
            loops: Vec::new(),
            slot_types: vec![None; function.local_types.len()],
            reference_origins: vec![None; function.local_types.len()],
            reference_mutability: vec![None; function.local_types.len()],
        };
        for parameter in &function.parameters {
            if let Type::Reference(_, mutable) = parameter.ty {
                guard.reference_origins[parameter.slot] = Some(parameter.slot);
                guard.reference_mutability[parameter.slot] = Some(mutable);
                guard.reference_scopes[0].push(parameter.slot);
            }
            for (slot, ty) in owned_slots(parameter.ty, parameter.slot, &ir.structs) {
                guard.available[slot] = true;
                guard.slot_types[slot] = Some(ty);
                if is_destructor
                    && matches!(ty, Type::Struct(id) if ir.structs[id].drop_function == Some(function_id))
                {
                    function.owned_slot_types[slot] = None;
                } else {
                    guard.scopes[0].push(slot);
                }
            }
        }
        let mut statements = std::mem::take(&mut function.statements);
        if let Some(value) = function.return_value.take() {
            statements.push(IrStatement::Return { value: Some(value) });
        }
        function.statements = guard.block(statements, false)?.0;
        for (slot, ty) in guard.slot_types.iter().copied().enumerate() {
            if let Some(ty) = ty
                && matches!(ty, Type::Struct(id) if ir.structs[id].drop_function.is_some())
            {
                function.owned_slot_types[slot] = Some(ty);
            }
        }
        if is_destructor {
            for parameter in &function.parameters {
                for (slot, ty) in owned_slots(parameter.ty, parameter.slot, &ir.structs) {
                    if matches!(ty, Type::Struct(id) if ir.structs[id].drop_function == Some(function_id))
                    {
                        function.owned_slot_types[slot] = None;
                    }
                }
            }
        }
    }
    Ok(())
}

struct LoopState {
    depth: usize,
    entry: Vec<bool>,
    breaks: Vec<Vec<bool>>,
    continues: Vec<Vec<bool>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BorrowOrigin {
    Local(usize),
    Unknown,
}

struct Guard<'a> {
    structs: &'a [RynStruct],
    reference_return_parameters: &'a [Option<usize>],
    available: Vec<bool>,
    moved_at: Vec<Span>,
    borrowed: Vec<usize>,
    scopes: Vec<Vec<usize>>,
    reference_scopes: Vec<Vec<usize>>,
    loops: Vec<LoopState>,
    slot_types: Vec<Option<Type>>,
    reference_origins: Vec<Option<usize>>,
    reference_mutability: Vec<Option<bool>>,
}

impl Guard<'_> {
    fn expression_span(&self, expression: &IrExpression) -> Span {
        match expression {
            IrExpression::Local { span, .. } | IrExpression::Field { span, .. } => *span,
            IrExpression::AddressOf { span, .. }
            | IrExpression::SliceElementAddress { span, .. } => *span,
            IrExpression::Call { arguments, .. } => arguments
                .first()
                .map_or(Span::default(), |value| self.expression_span(value)),
            IrExpression::Dereference { pointer, .. } => self.expression_span(pointer),
            IrExpression::ReferenceField { pointer, .. } => self.expression_span(pointer),
            _ => Span::default(),
        }
    }

    fn has_active_reference(&self, target: usize) -> bool {
        self.reference_origins
            .iter()
            .enumerate()
            .any(|(slot, origin)| {
                self.reference_mutability[slot].is_some()
                    && (*origin == Some(target) || origin.is_none())
            })
    }

    fn reference_origin(&self, expression: &IrExpression) -> Option<usize> {
        match expression {
            IrExpression::AddressOf {
                slot,
                pointer_type: Type::Reference(_, _),
                ..
            } => Some(*slot),
            IrExpression::Local {
                slot,
                ty: Type::Reference(_, _),
                ..
            } => self.reference_origins[*slot],
            IrExpression::Local {
                slot,
                ty: Type::Vec(_) | Type::Array(_) | Type::Slice(_),
                ..
            } => Some(*slot),
            IrExpression::ArrayAsSlice { array, .. } => self.reference_origin(array),
            IrExpression::If {
                then_value,
                else_value,
                ..
            } => {
                let then_origin = self.reference_origin(then_value)?;
                (self.reference_origin(else_value) == Some(then_origin)).then_some(then_origin)
            }
            IrExpression::Call {
                target: IrCallTarget::VecSlice(_),
                arguments,
                ..
            } => arguments
                .first()
                .and_then(|argument| self.reference_origin(argument)),
            IrExpression::Call {
                target: IrCallTarget::Function(function),
                arguments,
                ..
            } => self.reference_return_parameters[*function]
                .and_then(|parameter| arguments.get(parameter))
                .and_then(|argument| self.reference_origin(argument)),
            _ => None,
        }
    }

    fn is_borrowed_expression(&self, expression: &IrExpression) -> bool {
        match expression {
            IrExpression::AddressOf {
                pointer_type: Type::Reference(_, _),
                ..
            }
            | IrExpression::SliceElementAddress { .. }
            | IrExpression::Dereference { .. }
            | IrExpression::ReferenceField { .. }
            | IrExpression::StringAsStr(_) => true,
            IrExpression::Local {
                ty: Type::Reference(_, _) | Type::Slice(_),
                ..
            } => true,
            IrExpression::Call {
                target: IrCallTarget::VecSlice(_),
                ..
            } => true,
            IrExpression::Call {
                target: IrCallTarget::Function(function),
                return_type,
                ..
            } => {
                self.reference_return_parameters[*function].is_some()
                    || matches!(return_type, Type::Reference(_, _))
            }
            _ => false,
        }
    }

    fn explicit_reference(&self, expression: &IrExpression) -> Option<(BorrowOrigin, bool, Span)> {
        match expression {
            IrExpression::AddressOf {
                slot,
                pointer_type: Type::Reference(_, mutable),
                span,
                ..
            } => Some((BorrowOrigin::Local(*slot), *mutable, *span)),
            IrExpression::Local {
                slot,
                ty: Type::Reference(_, mutable),
                span,
            } => Some((
                self.reference_origins[*slot].map_or(BorrowOrigin::Unknown, BorrowOrigin::Local),
                *mutable,
                *span,
            )),
            IrExpression::Call {
                target: IrCallTarget::Function(function),
                arguments,
                return_type: Type::Reference(_, mutable),
            } => {
                let origin = self.reference_return_parameters[*function]
                    .and_then(|parameter| arguments.get(parameter))
                    .and_then(|argument| self.reference_origin(argument))
                    .map_or(BorrowOrigin::Unknown, BorrowOrigin::Local);
                Some((origin, *mutable, self.expression_span(expression)))
            }
            _ => None,
        }
    }

    fn local(&self, expression: &IrExpression) -> Option<(usize, Type, Span)> {
        match expression {
            IrExpression::Local { slot, ty, span } => Some((*slot, *ty, *span)),
            IrExpression::Field {
                value,
                struct_id,
                field_index,
                ty,
                span,
            } => {
                let (slot, _, _) = self.local(value)?;
                Some((
                    slot + self.structs[*struct_id].fields[*field_index].slot_offset,
                    *ty,
                    *span,
                ))
            }
            _ => None,
        }
    }

    fn expression(
        &mut self,
        expression: &mut IrExpression,
        consume: bool,
    ) -> Result<(), Diagnostic> {
        if let Some((slot, ty, span)) = self.local(expression) {
            let owners = owned_slots(ty, slot, self.structs);
            if owners.iter().any(|(slot, _)| !self.available[*slot]) {
                return Err(Diagnostic {
                    code: "R0240",
                    message: "use of a moved value".into(),
                    span,
                    help: Some(
                        "use `.clone()` before transferring ownership if both values are needed"
                            .into(),
                    ),
                });
            }
            if consume && !owners.is_empty() {
                if owners.iter().any(|(slot, _)| self.borrowed[*slot] > 0) {
                    return Err(Diagnostic { code: "R0242", message: "cannot move a value while an earlier operand borrows it".into(), span,
                        help: Some("complete the borrowing operation first, or pass an explicit `.clone()`".into()) });
                }
                if owners
                    .iter()
                    .any(|(slot, _)| self.has_active_reference(*slot))
                {
                    return Err(Diagnostic {
                        code: "R0249",
                        message: "cannot move a value while a reference to it is active".into(),
                        span,
                        help: Some("end the reference's scope before moving this value".into()),
                    });
                }
                for (owner, _) in owners {
                    self.available[owner] = false;
                    self.moved_at[owner] = span;
                }
                *expression = IrExpression::Move { slot, ty };
            } else {
                *expression = IrExpression::Local { slot, ty, span };
            }
            return Ok(());
        }
        match expression {
            IrExpression::StructValue { fields, .. } => {
                for (_, field) in fields {
                    self.expression(field, true)?;
                }
            }
            IrExpression::ArrayValue(values) => {
                for value in values {
                    self.expression(value, true)?;
                }
            }
            IrExpression::ArrayIndex { array, index, .. } => {
                self.expression(array, false)?;
                self.expression(index, false)?;
            }
            IrExpression::SliceIndex { slice, index, .. } => {
                self.expression(slice, false)?;
                self.expression(index, false)?;
            }
            IrExpression::SliceElementAddress { slice, index, .. } => {
                self.expression(slice, false)?;
                self.expression(index, false)?;
            }
            IrExpression::Field { value, .. } => {
                let custom_owner = matches!(
                    self.local(value).map(|(_, ty, _)| ty),
                    Some(Type::Struct(id)) if self.structs[id].drop_function.is_some()
                );
                self.expression(value, !custom_owner)?;
            }
            IrExpression::ReferenceField { pointer, .. } => {
                self.expression(pointer, false)?;
            }
            IrExpression::Call {
                target, arguments, ..
            } => self.arguments(*target, arguments)?,
            IrExpression::If {
                condition,
                then_value,
                else_value,
                ty,
            } => {
                self.expression(condition, false)?;
                let before = self.available.clone();
                let origins_before = self.reference_origins.clone();
                let own = !owned_slots(*ty, 0, self.structs).is_empty();
                self.expression(then_value, consume || own)?;
                let then_state = self.available.clone();
                let then_origins = self.reference_origins.clone();
                self.available = before;
                self.reference_origins = origins_before;
                self.expression(else_value, consume || own)?;
                let else_origins = self.reference_origins.clone();
                self.intersect(&then_state);
                for (current, (then, else_)) in self
                    .reference_origins
                    .iter_mut()
                    .zip(then_origins.into_iter().zip(else_origins))
                {
                    *current = if then == else_ { then } else { None };
                }
            }
            IrExpression::EnumMatch {
                value, arms, ty, ..
            } => {
                self.expression(value, true)?;
                let before_arms = self.available.clone();
                let origins_before_arms = self.reference_origins.clone();
                let result_owns = !owned_slots(*ty, 0, self.structs).is_empty();
                let mut merged = None::<Vec<bool>>;
                let mut merged_origins = None::<Vec<Option<usize>>>;
                for arm in arms {
                    self.available.clone_from(&before_arms);
                    self.reference_origins.clone_from(&origins_before_arms);
                    let mut arm_slots = Vec::new();
                    for binding in &arm.bindings {
                        for (slot, field_ty) in owned_slots(binding.ty, binding.slot, self.structs)
                        {
                            self.available[slot] = true;
                            self.slot_types[slot] = Some(field_ty);
                            arm_slots.push(slot);
                        }
                    }
                    self.expression(&mut arm.body, consume || result_owns)?;
                    let arm_origins = self.reference_origins.clone();
                    for slot in arm_slots {
                        self.available[slot] = false;
                    }
                    if let Some(state) = &mut merged {
                        self.intersect(state);
                        *state = self.available.clone();
                    } else {
                        merged = Some(self.available.clone());
                    }
                    if let Some(origins) = &mut merged_origins {
                        for (origin, arm_origin) in origins.iter_mut().zip(arm_origins) {
                            if *origin != arm_origin {
                                *origin = None;
                            }
                        }
                    } else {
                        merged_origins = Some(arm_origins);
                    }
                }
                if let Some(state) = merged {
                    self.available = state;
                }
                if let Some(origins) = merged_origins {
                    self.reference_origins = origins;
                }
            }
            IrExpression::Propagate { value, .. } => self.expression(value, true)?,
            IrExpression::StringAsStr(value) => self.expression(value, false)?,
            IrExpression::Binary { left, right, .. } => {
                self.expression(left, false)?;
                let loan = self.borrow_result(left);
                self.expression(right, false)?;
                self.release(loan);
            }
            IrExpression::Negate(value, _)
            | IrExpression::Not(value)
            | IrExpression::BitNot(value, _)
            | IrExpression::Cast { value, .. } => self.expression(value, false)?,
            _ => {}
        }
        Ok(())
    }

    fn arguments(
        &mut self,
        target: IrCallTarget,
        arguments: &mut [IrExpression],
    ) -> Result<(), Diagnostic> {
        let consume_all = matches!(
            target,
            IrCallTarget::Function(_) | IrCallTarget::EnumNew { .. }
        );
        let mut loans = Vec::new();
        let mut explicit_borrows = Vec::<(BorrowOrigin, bool)>::new();
        for (index, argument) in arguments.iter_mut().enumerate() {
            let consume = consume_all
                || matches!(
                    target,
                    IrCallTarget::IndirectFunctionPointer(signature_id)
                        if index > 0
                            && crate::sema::function_pointer_info(signature_id)
                                .parameters
                                .get(index - 1)
                                .is_some_and(|ty| !owned_slots(*ty, 0, self.structs).is_empty())
                )
                || matches!(target, IrCallTarget::Vec(op, _) if op.consumes_argument(index))
                || matches!(target, IrCallTarget::Map(op, _) if op.consumes_argument(index))
                || matches!(target, IrCallTarget::System(op) if op.consumes_argument(index));
            self.expression(argument, consume)?;
            if index == 0
                && let IrCallTarget::Vec(operation, _) = target
                && (operation.mutates() || operation == crate::vector_ops::VecOp::Drop)
                && let Some((slot, _, _)) = self.local(argument)
                && (self.has_active_reference(slot) || self.borrowed[slot] > 0)
            {
                return Err(Diagnostic {
                    code: "R0249",
                    message: "cannot mutate or drop a Vec while one of its elements is borrowed"
                        .into(),
                    span: self
                        .local(argument)
                        .map_or(Span::default(), |(_, _, span)| span),
                    help: Some(
                        "end the element reference's scope before mutating or dropping the Vec"
                            .into(),
                    ),
                });
            }
            if let Some((origin, mutable, span)) = self.explicit_reference(argument) {
                if explicit_borrows
                    .iter()
                    .any(|(previous_origin, previous_mutable)| {
                        (*previous_origin == origin
                            || matches!(origin, BorrowOrigin::Unknown)
                            || matches!(previous_origin, BorrowOrigin::Unknown))
                            && (*previous_mutable || mutable)
                    })
                {
                    return Err(Diagnostic {
                        code: "R0249",
                        message: "conflicting references to the same value in one call".into(),
                        span,
                        help: Some("avoid overlapping a mutable borrow with any other reference to this value".into()),
                    });
                }
                let binding_slot = match argument {
                    IrExpression::Local {
                        slot,
                        ty: Type::Reference(_, _),
                        ..
                    } => Some(*slot),
                    _ => None,
                };
                if let BorrowOrigin::Local(target) = origin
                    && self.reference_origins.iter().enumerate().any(
                        |(active_slot, active_origin)| {
                            Some(active_slot) != binding_slot
                                && (*active_origin == Some(target)
                                    || (active_origin.is_none()
                                        && self.reference_mutability[active_slot].is_some()))
                                && (self.reference_mutability[active_slot].unwrap_or(true)
                                    || mutable)
                        },
                    )
                {
                    return Err(Diagnostic {
                        code: "R0249",
                        message: "conflicting references to the same value in one call".into(),
                        span,
                        help: Some(
                            "end the earlier reference's scope before borrowing this value again"
                                .into(),
                        ),
                    });
                }
                explicit_borrows.push((origin, mutable));
            }
            if !consume || self.is_borrowed_expression(argument) {
                loans.extend(self.borrow_result(argument));
            }
        }
        self.release(loans);
        Ok(())
    }

    fn borrow_result(&mut self, expression: &IrExpression) -> Vec<usize> {
        if let IrExpression::Call {
            target: IrCallTarget::VecSlice(_),
            arguments,
            ..
        } = expression
            && let Some(vector) = arguments.first()
        {
            return self.borrow_result(vector);
        }
        if let IrExpression::Call {
            target: IrCallTarget::Function(function),
            arguments,
            return_type,
            ..
        } = expression
        {
            if let Some(parameter) = self.reference_return_parameters[*function]
                && let Some(source) = arguments.get(parameter)
            {
                return self.borrow_result(source);
            }
            if matches!(return_type, Type::Reference(_, _)) {
                return arguments
                    .iter()
                    .flat_map(|argument| self.borrow_result(argument))
                    .collect();
            }
        }
        if let IrExpression::StringAsStr(value) = expression {
            return self.borrow_result(value);
        }
        if let IrExpression::SliceElementAddress { slice, .. } = expression {
            return self.borrow_result(slice);
        }
        let slots = self
            .local(expression)
            .map(|(slot, ty, _)| owned_slots(ty, slot, self.structs))
            .unwrap_or_default();
        for (slot, _) in &slots {
            self.borrowed[*slot] += 1;
        }
        slots.into_iter().map(|(slot, _)| slot).collect()
    }

    fn release(&mut self, slots: Vec<usize>) {
        for slot in slots {
            self.borrowed[slot] -= 1;
        }
    }

    fn intersect(&mut self, other: &[bool]) {
        for (current, other) in self.available.iter_mut().zip(other) {
            *current &= *other;
        }
    }

    fn drops(&mut self, depth: usize) -> IrStatement {
        for slot in self.reference_scopes[depth..].iter().flatten().copied() {
            self.reference_origins[slot] = None;
            self.reference_mutability[slot] = None;
        }
        let slots = self.scopes[depth..]
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev().copied())
            .collect::<Vec<_>>();
        for slot in &slots {
            self.available[*slot] = false;
        }
        IrStatement::Drop {
            slots: slots
                .into_iter()
                .map(|slot| {
                    (
                        slot,
                        self.slot_types[slot].expect("owned slot carries a leaf type"),
                    )
                })
                .collect(),
        }
    }

    fn backedge(&self, state: &[bool], entry: &[bool]) -> Result<(), Diagnostic> {
        if let Some(slot) = entry
            .iter()
            .zip(state)
            .position(|(before, after)| *before && !*after)
        {
            return Err(Diagnostic {
                code: "R0241",
                message: "a moved value would be reused on the next loop iteration".into(),
                span: self.moved_at[slot],
                help: Some(
                    "replace the moved value before continuing, or pass an explicit `.clone()`"
                        .into(),
                ),
            });
        }
        Ok(())
    }

    fn block(
        &mut self,
        statements: Vec<IrStatement>,
        nested: bool,
    ) -> Result<(Vec<IrStatement>, bool), Diagnostic> {
        if nested {
            self.scopes.push(Vec::new());
            self.reference_scopes.push(Vec::new());
        }
        let mut output = Vec::new();
        let mut falls_through = true;
        for mut statement in statements {
            if !falls_through {
                break;
            }
            match &mut statement {
                IrStatement::Let { ty, value, .. } | IrStatement::Assign { ty, value, .. }
                    if matches!(
                        &*value,
                        IrExpression::Dereference { ty: pointee, .. }
                            if matches!(
                                pointee,
                                Type::OwnedString | Type::Vec(_) | Type::Map(_) | Type::Enum(_)
                            )
                    ) =>
                {
                    let _ = ty;
                    return Err(Diagnostic {
                        code: "R0240",
                        message: "cannot move a borrowed view out of a reference".into(),
                        span: self.expression_span(value),
                        help: Some("clone the value explicitly with `.clone()`".into()),
                    });
                }
                IrStatement::Block(statements) => {
                    let (output, falls) = self.block(std::mem::take(statements), true)?;
                    *statements = output;
                    falls_through = falls;
                }
                IrStatement::Let {
                    slot,
                    ty,
                    value,
                    span,
                } => {
                    self.expression(value, true)?;
                    if let Type::Reference(_, mutable) = ty {
                        let Some(origin) = self.reference_origin(value) else {
                            return Err(Diagnostic {
                                code: "R0250",
                                message: "cannot store a reference with unknown lifetime provenance".into(),
                                span: *span,
                                help: Some("bind references only when their source is a known local or parameter".into()),
                            });
                        };
                        self.reference_origins[*slot] = Some(origin);
                        self.reference_mutability[*slot] = Some(*mutable);
                        self.reference_scopes.last_mut().unwrap().push(*slot);
                    }
                    for (owner, leaf_ty) in owned_slots(*ty, *slot, self.structs) {
                        self.available[owner] = true;
                        self.slot_types[owner] = Some(leaf_ty);
                        self.scopes.last_mut().unwrap().push(owner);
                    }
                }
                IrStatement::Assign {
                    slot,
                    ty,
                    value,
                    span,
                } => {
                    self.expression(value, true)?;
                    if !matches!(ty, Type::Reference(_, _)) && self.has_active_reference(*slot) {
                        return Err(Diagnostic {
                            code: "R0249",
                            message: "cannot assign to a value while it is borrowed".into(),
                            span: *span,
                            help: Some("write through the active mutable reference, or end the reference's scope first".into()),
                        });
                    }
                    if let Type::Reference(_, mutable) = ty {
                        self.reference_origins[*slot] = self.reference_origin(value);
                        self.reference_mutability[*slot] = Some(*mutable);
                    }
                    for (owner, leaf_ty) in owned_slots(*ty, *slot, self.structs) {
                        self.available[owner] = true;
                        self.slot_types[owner] = Some(leaf_ty);
                    }
                }
                IrStatement::FieldAssign { slot, ty, value } => {
                    if matches!(
                        self.slot_types.get(*slot).copied().flatten(),
                        Some(Type::Struct(id)) if self.structs[id].drop_function.is_some()
                    ) {
                        return Err(Diagnostic {
                            code: "R0255",
                            message: "cannot overwrite the resource handle field of a custom-destructor value".into(),
                            span: self.expression_span(value),
                            help: Some("replace the whole value so Ryn Guard can run its destructor first".into()),
                        });
                    }
                    self.expression(value, true)?;
                    for (owner, leaf_ty) in owned_slots(*ty, *slot, self.structs) {
                        self.available[owner] = true;
                        self.slot_types[owner] = Some(leaf_ty);
                    }
                }
                IrStatement::ArrayAssign {
                    slot,
                    index,
                    value,
                    span,
                    ..
                } => {
                    if self.has_active_reference(*slot) {
                        return Err(Diagnostic {
                            code: "R0249",
                            message: "cannot modify an array while an element is borrowed".into(),
                            span: *span,
                            help: Some(
                                "end the element reference's scope before modifying the array"
                                    .into(),
                            ),
                        });
                    }
                    self.expression(index, false)?;
                    self.expression(value, true)?;
                }
                IrStatement::DereferenceAssign { pointer, value, .. } => {
                    self.expression(pointer, false)?;
                    self.expression(value, false)?;
                }
                IrStatement::ReferenceFieldAssign {
                    pointer,
                    struct_id,
                    field_index,
                    value,
                    ..
                } => {
                    // Receiver aliasing conflicts are rejected at the call site;
                    // inside the method, mutating through `mut self` is the point.
                    self.expression(pointer, false)?;
                    if matches!(
                        self.structs
                            .get(*struct_id)
                            .and_then(|definition| definition.fields.get(*field_index))
                            .map(|field| field.ty),
                        Some(Type::Struct(id)) if self.structs[id].drop_function.is_some()
                    ) {
                        let field_span = self.expression_span(pointer);
                        return Err(Diagnostic {
                            code: "R0255",
                            message: "cannot overwrite the resource handle field of a custom-destructor value".into(),
                            span: field_span,
                            help: Some("replace the whole value so Ryn Guard can run its destructor first".into()),
                        });
                    }
                    self.expression(value, true)?;
                }
                IrStatement::Print { value, .. } => self.expression(value, false)?,
                IrStatement::PrintTemplate(parts) => {
                    for part in parts {
                        if let IrPrintPart::Value { value, .. } = part {
                            self.expression(value, false)?;
                        }
                    }
                }
                IrStatement::Call { target, arguments } => self.arguments(*target, arguments)?,
                IrStatement::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    self.expression(condition, false)?;
                    let before = self.available.clone();
                    let origins_before = self.reference_origins.clone();
                    let (then_result, then_falls) = self.block(std::mem::take(then_body), true)?;
                    *then_body = then_result;
                    let then_state = self.available.clone();
                    let then_origins = self.reference_origins.clone();
                    self.available = before;
                    self.reference_origins = origins_before.clone();
                    let (else_result, else_falls) = self.block(std::mem::take(else_body), true)?;
                    *else_body = else_result;
                    let else_origins = self.reference_origins.clone();
                    if then_falls && else_falls {
                        self.intersect(&then_state);
                        self.reference_origins = then_origins
                            .iter()
                            .zip(&else_origins)
                            .map(|(left, right)| if left == right { *left } else { None })
                            .collect();
                    } else if then_falls {
                        self.available = then_state;
                        self.reference_origins = then_origins;
                    } else if else_falls {
                        self.reference_origins = else_origins;
                    }
                    falls_through = then_falls || else_falls;
                }
                IrStatement::While { condition, body } => {
                    let before = self.available.clone();
                    self.expression(condition, false)?;
                    *body = self.loop_body(std::mem::take(body), before)?;
                }
                IrStatement::For {
                    start, end, body, ..
                } => {
                    self.expression(start, false)?;
                    self.expression(end, false)?;
                    *body = self.loop_body(std::mem::take(body), self.available.clone())?;
                }
                IrStatement::Break | IrStatement::Continue => {
                    let index = self.loops.len() - 1;
                    let state = self.available.clone();
                    let depth = self.loops[index].depth;
                    if matches!(statement, IrStatement::Break) {
                        self.loops[index].breaks.push(state);
                    } else {
                        self.loops[index].continues.push(state);
                    }
                    output.push(self.drops(depth));
                    falls_through = false;
                }
                IrStatement::Return { value } => {
                    if let Some(value) = value {
                        self.expression(value, true)?;
                    }
                    // The returned expression must be evaluated before cleanup.
                    // Codegen performs the all-scope drop after compiling its result.
                    falls_through = false;
                }
                IrStatement::Drop { .. } => {}
            }
            output.push(statement);
        }
        if falls_through {
            output.push(self.drops(self.scopes.len() - 1));
        }
        if nested {
            self.scopes.pop();
            self.reference_scopes.pop();
        }
        Ok((output, falls_through))
    }

    fn loop_body(
        &mut self,
        body: Vec<IrStatement>,
        backedge: Vec<bool>,
    ) -> Result<Vec<IrStatement>, Diagnostic> {
        let entry = self.available.clone();
        let entry_origins = self.reference_origins.clone();
        self.loops.push(LoopState {
            depth: self.scopes.len(),
            entry: backedge,
            breaks: Vec::new(),
            continues: Vec::new(),
        });
        let (body, falls) = self.block(body, true)?;
        let state = self.loops.pop().unwrap();
        if falls {
            self.backedge(&self.available, &state.entry)?;
        }
        for continuation in &state.continues {
            self.backedge(continuation, &state.entry)?;
        }
        self.available = entry;
        for (current, before) in self.reference_origins.iter_mut().zip(entry_origins) {
            if *current != before {
                *current = None;
            }
        }
        for exit in state.breaks {
            self.intersect(&exit);
        }
        Ok(body)
    }
}
