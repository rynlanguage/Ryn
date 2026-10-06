path = 'src/sema.rs'
src = open(path, encoding='utf-8').read()

# --- FieldAssign: reference root -> ReferenceFieldAssign ---
old = '''                if !binding.mutable {
                    let error = diag("R0204", format!("`{object}` is immutable"), object_span)
                        .with_help(format!(
                            "declare `mut {object} := ...` if you intend to change its fields"
                        ));
                    self.check_discarded_expression(value);
                    return Err(error);
                }
                let Some(((final_field, final_field_span), parent_fields)) = fields.split_last()
                else {'''
new = '''                if !binding.mutable {
                    let error = diag("R0204", format!("`{object}` is immutable"), object_span)
                        .with_help(format!(
                            "declare `mut {object} := ...` if you intend to change its fields"
                        ));
                    self.check_discarded_expression(value);
                    return Err(error);
                }
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
                        &fields,
                        value,
                        span,
                    );
                }
                let Some(((final_field, final_field_span), parent_fields)) = fields.split_last()
                else {'''
assert old in src, "fieldassign mutable check"
src = src.replace(old, new)

# --- the reference_field_assign helper (before check_field_visibility) ---
old = '''    fn check_field_visibility('''
new = '''    fn reference_field_assign(
        &mut self,
        pointer: IrExpression,
        pointee: Type,
        reference_mutable: bool,
        fields: &[String],
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
            return Err(diag(
                "R0206",
                "cannot assign through a shared reference",
                span,
            )
            .with_help("the receiver must be borrowed with `mut self` to mutate fields"));
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
        let value_span = value.span();
        let (value, actual) = self.expression(value, Some(field_info.ty))?;
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

    fn check_field_visibility('''
assert old in src, "check_field_visibility anchor"
src = src.replace(old, new, 1)

# --- const fallback in EnumConstruct handling: find the EnumConstruct arm ---
# The parser lowers bare `Player::MAX_HEALTH` to EnumConstruct { enum_name: "Player", variant: "MAX_HEALTH" }.
# In sema's EnumConstruct: if the enum name does not resolve, try a zero-arg function.
old = '''            Expression::EnumConstruct {'''
idx = src.find(old)
assert idx != -1, "enum construct arm"
# Find the point where the enum is resolved; insert a fallback before "unknown enum" diagnostics.
# Locate the unknown-enum diagnostic within the EnumConstruct arm.
arm_end = src.find("Expression::EnumNew", idx)
segment = src[idx:idx+9000]
marker = None
for candidate in [
    'unknown enum',
    'is not an enum',
]:
    position = segment.find(candidate)
    if position != -1:
        marker = (candidate, position)
        break
print("marker:", marker)
open(path, 'w', encoding='utf-8', newline='').write(src)
print('sema part 3a done')
