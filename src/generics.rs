//! Call-site specialization for generic functions.
//!
//! The native backend has concrete layouts, so this pass clones a generic
//! function for each inferred type-argument tuple and substitutes its
//! signatures and local annotations before semantic analysis.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
};

use crate::{
    ast::{
        BinaryOp, EnumDef, Expression, Function, PrintPart, Program, Statement, StructDef,
        StructField, TypeName, VariantDef,
    },
    source::{Diagnostic, Span},
};

const MAX_SPECIALIZATIONS: usize = 256;

const STRUCT_PLACEHOLDER: &str = "$RynStructParam#";
const STRUCT_INSTANCE: &str = "$RynStruct#";
const ENUM_PLACEHOLDER: &str = "$RynEnumParam#";
const ENUM_INSTANCE: &str = "$RynEnum#";
const TUPLE_PREFIX: &str = "$RynTuple#";
const OPTION_PREFIX: &str = "$RynOption#";

thread_local! {
    /// The structure declarations of the program being specialized, for unifying a generic
    /// structure written with type parameters against a concrete instance. Tuple types the
    /// specialization needs are added here as well.
    static STRUCTS: RefCell<Vec<StructDef>> = const { RefCell::new(Vec::new()) };
    /// Enum declarations created while specializing (the `Option` instances of constructor
    /// arguments). They are appended to the program's enums.
    static NEW_ENUMS: RefCell<Vec<EnumDef>> = const { RefCell::new(Vec::new()) };
}

fn find_struct(name: &str) -> Option<StructDef> {
    STRUCTS.with(|structs| structs.borrow().iter().find(|s| s.name == name).cloned())
}

/// Finds an enum among the program's enums or among those created while specializing.
fn find_enum_definition(name: &str, enum_definitions: &[EnumDef]) -> Option<EnumDef> {
    enum_definitions
        .iter()
        .find(|definition| definition.name == name)
        .cloned()
        .or_else(|| {
            NEW_ENUMS.with(|enums| {
                enums
                    .borrow()
                    .iter()
                    .find(|definition| definition.name == name)
                    .cloned()
            })
        })
}

/// The tuple structure with the given element types, registered on first use. Its name and
/// layout match the parser's tuple types, so semantic analysis reuses it.
fn tuple_type(fields: Vec<TypeName>, span: Span) -> TypeName {
    let key = fields.iter().map(type_key).collect::<Vec<_>>().join(",");
    let name = format!("{TUPLE_PREFIX}{key}");
    STRUCTS.with(|structs| {
        let mut structs = structs.borrow_mut();
        if !structs.iter().any(|definition| definition.name == name) {
            structs.push(StructDef {
                derives: Vec::new(),
                name: name.clone(),
                type_parameters: Vec::new(),
                fields: fields
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| StructField {
                        name: format!("_{index}"),
                        ty,
                        public: false,
                        span,
                    })
                    .collect(),
                public: false,
                repr_c: false,
                drop_function: None,
                module_path: String::new(),
                display: String::new(),
                span,
            });
        }
    });
    TypeName::Named(name, span)
}

/// The `Option` instance whose `Some` payload is `payload`, created when the program has none.
fn option_type(payload: TypeName, span: Span, enum_definitions: &[EnumDef]) -> TypeName {
    let is_instance = |definition: &EnumDef| {
        definition.name.starts_with(OPTION_PREFIX)
            && definition.variants.len() == 2
            && definition.variants[0].name == "Some"
            && definition.variants[0].fields.len() == 1
            && type_key(&definition.variants[0].fields[0]) == type_key(&payload)
            && definition.variants[1].name == "None"
            && definition.variants[1].fields.is_empty()
    };
    if let Some(definition) = enum_definitions
        .iter()
        .find(|definition| is_instance(definition))
    {
        return TypeName::Named(definition.name.clone(), span);
    }
    let name = NEW_ENUMS.with(|enums| {
        let mut enums = enums.borrow_mut();
        if let Some(definition) = enums.iter().find(|definition| is_instance(definition)) {
            return definition.name.clone();
        }
        let name = format!("{OPTION_PREFIX}generic{}", enums.len());
        enums.push(EnumDef {
            name: name.clone(),
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Some".into(),
                    fields: vec![payload.clone()],
                    span,
                },
                VariantDef {
                    name: "None".into(),
                    fields: Vec::new(),
                    span,
                },
            ],
            public: false,
            module_path: String::new(),
            span,
        });
        name
    });
    TypeName::Named(name, span)
}

/// The instance of a generic user enum that a constructor call `Enum::Variant(payload)` builds,
/// found through the enum's placeholder (`$RynEnumParam#<Enum>#<n>`). The placeholder's
/// parameters are inferred from the payload; the instance is reused or created.
fn generic_enum_instance(
    enum_name: &str,
    variant: &str,
    payload: &[TypeName],
    span: Span,
    enum_definitions: &[EnumDef],
) -> Option<TypeName> {
    let placeholder_prefix = format!("{ENUM_PLACEHOLDER}{enum_name}#");
    let placeholder = enum_definitions.iter().find(|definition| {
        definition.name.starts_with(&placeholder_prefix)
            && definition
                .variants
                .iter()
                .any(|candidate| candidate.name == variant)
    })?;
    let fields = &placeholder
        .variants
        .iter()
        .find(|candidate| candidate.name == variant)?
        .fields;
    if fields.len() != payload.len() {
        return None;
    }
    let mut substitutions = HashMap::new();
    for (pattern, actual) in fields.iter().zip(payload) {
        let mut additions = HashMap::new();
        unify(pattern, actual, &mut additions, enum_definitions, span).ok()?;
        merge_substitutions(&mut substitutions, additions, span).ok()?;
    }
    let variants = placeholder
        .variants
        .iter()
        .map(|candidate| VariantDef {
            name: candidate.name.clone(),
            fields: candidate
                .fields
                .iter()
                .map(|field| {
                    let mut ty = field.clone();
                    substitute_type(&mut ty, &substitutions);
                    ty
                })
                .collect(),
            span,
        })
        .collect::<Vec<_>>();
    if variants
        .iter()
        .any(|candidate| candidate.fields.iter().any(type_has_parameter))
    {
        return None;
    }
    let instance_prefix = format!("{ENUM_INSTANCE}{enum_name}#");
    let same_variants = |definition: &EnumDef| {
        definition.name.starts_with(&instance_prefix)
            && definition.variants.len() == variants.len()
            && definition
                .variants
                .iter()
                .zip(&variants)
                .all(|(left, right)| {
                    left.name == right.name
                        && left.fields.len() == right.fields.len()
                        && left
                            .fields
                            .iter()
                            .zip(&right.fields)
                            .all(|(left, right)| type_key(left) == type_key(right))
                })
    };
    if let Some(definition) = enum_definitions
        .iter()
        .chain(&NEW_ENUMS.with(|enums| enums.borrow().clone()))
        .find(|definition| same_variants(definition))
    {
        return Some(TypeName::Named(definition.name.clone(), span));
    }
    let name = NEW_ENUMS.with(|enums| {
        let mut enums = enums.borrow_mut();
        let name = format!("{instance_prefix}generic{}", enums.len());
        enums.push(EnumDef {
            name: name.clone(),
            type_parameters: Vec::new(),
            variants: variants.clone(),
            public: false,
            module_path: String::new(),
            span,
        });
        name
    });
    Some(TypeName::Named(name, span))
}

/// Whether a tuple structure still mentions type parameters. Such tuples are templates for
/// generic functions and are not emitted.
fn is_tuple_template(definition: &StructDef) -> bool {
    definition.name.starts_with(TUPLE_PREFIX)
        && definition
            .fields
            .iter()
            .any(|field| type_has_parameter(&field.ty))
}

/// Whether an enum placeholder of a generic user enum (`$RynEnumParam#<Template>#<n>`) stands
/// for the concrete instance `$RynEnum#<Template>#<k>`.
fn enum_placeholder_matches(pattern: &str, actual: &str) -> bool {
    let (Some(pattern), Some(actual)) = (
        pattern.strip_prefix(ENUM_PLACEHOLDER),
        actual.strip_prefix(ENUM_INSTANCE),
    ) else {
        return false;
    };
    let template = |name: &str| {
        name.rsplit_once('#')
            .map(|(template, _)| template.to_owned())
    };
    template(pattern).is_some() && template(pattern) == template(actual)
}

/// Specializes every generic function for the type arguments its call sites
/// infer and removes the generic templates. A program without generic
/// functions is returned unchanged.
pub fn monomorphize(program: Program) -> Result<Program, Diagnostic> {
    STRUCTS.with(|structs| *structs.borrow_mut() = program.structs.clone());
    NEW_ENUMS.with(|enums| enums.borrow_mut().clear());
    let result = specialize_program(program);
    let structs = STRUCTS.with(|structs| std::mem::take(&mut *structs.borrow_mut()));
    let mut program = result?;
    program.structs = structs
        .into_iter()
        .filter(|definition| {
            !definition.name.starts_with(STRUCT_PLACEHOLDER) && !is_tuple_template(definition)
        })
        .collect();
    // Enum placeholders serve only the specializer's unification, like struct placeholders.
    program
        .enums
        .retain(|definition| !definition.name.starts_with(ENUM_PLACEHOLDER));
    Ok(program)
}

fn specialize_program(mut program: Program) -> Result<Program, Diagnostic> {
    let enum_definitions = program.enums.clone();
    let generic_functions = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| !function.type_parameters.is_empty())
        .map(|(index, function)| (index, function.clone()))
        .collect::<Vec<_>>();
    if generic_functions.is_empty() {
        return Ok(program);
    }
    for (_, function) in &generic_functions {
        check_bounded_method_calls(function, &program.shapes)?;
    }

    let mut templates = HashMap::<String, Function>::new();
    for (_, function) in &generic_functions {
        templates.insert(function.name.clone(), function.clone());
    }
    for import in &program.uses {
        let module_path = import.path.join("::");
        let Some(alias) = import.local_name() else {
            continue;
        };
        let prefix = format!("{module_path}::");
        for (_, function) in &generic_functions {
            if !function.public || function.module_path != module_path {
                continue;
            }
            if let Some(suffix) = function.name.strip_prefix(&prefix) {
                templates.insert(format!("{alias}::{suffix}"), function.clone());
            }
        }
    }

    let mut concrete_functions = program
        .functions
        .drain(..)
        .filter(|function| function.type_parameters.is_empty())
        .collect::<Vec<_>>();
    let mut known_functions = concrete_functions
        .iter()
        .map(|function| (function.name.clone(), function.clone()))
        .collect::<HashMap<_, _>>();
    let mut specializations = HashMap::<String, String>::new();
    let mut pending = Vec::<Function>::new();

    for function in &mut concrete_functions {
        let mut environment = parameter_environment(function);
        let caller = function.clone();
        rewrite_statements(
            &mut function.body,
            &mut environment,
            &caller,
            &templates,
            &known_functions,
            &enum_definitions,
            &mut specializations,
            &mut pending,
        )?;
        if let Some(value) = &mut function.return_value {
            rewrite_expression(
                value,
                &environment,
                &caller,
                &templates,
                &known_functions,
                &enum_definitions,
                &mut specializations,
                &mut pending,
            )?;
        }
    }

    let mut next_id = 0usize;
    let mut cursor = 0usize;
    while cursor < pending.len() {
        let mut function = pending[cursor].clone();
        cursor += 1;
        let mut environment = parameter_environment(&function);
        let caller = function.clone();
        rewrite_statements(
            &mut function.body,
            &mut environment,
            &caller,
            &templates,
            &known_functions,
            &enum_definitions,
            &mut specializations,
            &mut pending,
        )?;
        if let Some(value) = &mut function.return_value {
            rewrite_expression(
                value,
                &environment,
                &caller,
                &templates,
                &known_functions,
                &enum_definitions,
                &mut specializations,
                &mut pending,
            )?;
        }
        known_functions.insert(function.name.clone(), function.clone());
        concrete_functions.push(function);
        if concrete_functions.len() > MAX_SPECIALIZATIONS {
            return Err(Diagnostic {
                code: "R0261",
                message: "generic specialization limit exceeded".into(),
                span: concrete_functions
                    .last()
                    .map_or(Span::default(), |f| f.span),
                help: Some(
                    "reduce recursively growing type arguments or split the generic call graph"
                        .into(),
                ),
            });
        }
        next_id = next_id.max(cursor);
    }
    let _ = next_id;
    program.functions = concrete_functions;
    program
        .enums
        .extend(NEW_ENUMS.with(|enums| std::mem::take(&mut *enums.borrow_mut())));
    program.enums.retain(|definition| {
        !enum_has_type_parameters(definition) && !definition.name.starts_with(ENUM_PLACEHOLDER)
    });
    Ok(program)
}

/// A generic body is analyzed only after it has been specialized, when `T` has become a concrete
/// type that may have more methods than the shape it is bounded by. Check the body against the
/// bound here: a method called on a parameter of type `T` must belong to a shape of `T`'s bound.
fn check_bounded_method_calls(
    function: &Function,
    shapes: &[crate::ast::ShapeDef],
) -> Result<(), Diagnostic> {
    // parameter name -> (type parameter, allowed methods, shape names)
    let mut bounded = HashMap::<String, (String, HashSet<String>, Vec<String>)>::new();
    for parameter in &function.parameters {
        let TypeName::Parameter(type_parameter, _) = &parameter.ty else {
            continue;
        };
        let Some((_, bounds)) = function
            .type_parameter_bounds
            .iter()
            .find(|(name, _)| name == type_parameter)
        else {
            continue;
        };
        let mut allowed = HashSet::new();
        let mut known = true;
        for bound in bounds {
            let Some(shape) = shapes.iter().find(|shape| {
                shape.name == *bound
                    || shape.name.rsplit("::").next()
                        == Some(bound.rsplit("::").next().unwrap_or(bound))
            }) else {
                known = false;
                break;
            };
            allowed.extend(shape.methods.iter().map(|method| method.name.clone()));
        }
        if known && !bounds.is_empty() {
            bounded.insert(
                parameter.name.clone(),
                (type_parameter.clone(), allowed, bounds.clone()),
            );
        }
    }
    if bounded.is_empty() {
        return Ok(());
    }
    // A local that reuses a parameter's name is no longer the bounded value.
    let mut body = function.body.clone();
    let mut rebound = HashSet::<String>::new();
    fn collect_lets(list: &[Statement], names: &mut HashSet<String>) {
        for statement in list {
            match statement {
                Statement::Let { name, .. } => {
                    names.insert(name.clone());
                }
                Statement::For { name, body, .. } | Statement::ForEach { name, body, .. } => {
                    names.insert(name.clone());
                    collect_lets(body, names);
                }
                Statement::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    collect_lets(then_body, names);
                    collect_lets(else_body, names);
                }
                Statement::While { body, .. } | Statement::Defer { body, .. } => {
                    collect_lets(body, names);
                }
                _ => {}
            }
        }
    }
    collect_lets(&body, &mut rebound);
    let violation = |value: &Expression, method: &str, span: Span| -> Option<Diagnostic> {
        let Expression::Name(name, _) = value else {
            return None;
        };
        if rebound.contains(name) {
            return None;
        }
        let (type_parameter, allowed, shapes) = bounded.get(name)?;
        if allowed.contains(method) {
            return None;
        }
        Some(Diagnostic {
            code: "R0450",
            message: format!(
                "method `{method}` is not part of the bound `{}` of type parameter `{type_parameter}`",
                shapes.join(" + ")
            ),
            span,
            help: Some(format!(
                "a generic function may only call the methods its shape bound declares; add `{method}` to the shape"
            )),
        })
    };
    let mut found: Option<Diagnostic> = None;
    fn statement_calls(
        list: &[Statement],
        check: &dyn Fn(&Expression, &str, Span) -> Option<Diagnostic>,
        found: &mut Option<Diagnostic>,
    ) {
        for statement in list {
            match statement {
                Statement::MethodCall {
                    value, name, span, ..
                } => {
                    if found.is_none() {
                        *found = check(value, name, *span);
                    }
                }
                Statement::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    statement_calls(then_body, check, found);
                    statement_calls(else_body, check, found);
                }
                Statement::While { body, .. }
                | Statement::Defer { body, .. }
                | Statement::For { body, .. }
                | Statement::ForEach { body, .. } => statement_calls(body, check, found),
                _ => {}
            }
        }
    }
    statement_calls(&body, &violation, &mut found);
    let mut visit = |expression: &mut Expression| -> Result<(), Diagnostic> {
        if found.is_none()
            && let Expression::MethodCall {
                value, name, span, ..
            } = expression
        {
            found = violation(value, name, *span);
        }
        Ok(())
    };
    crate::visit::visit_statements(&mut body, &mut visit)?;
    if let Some(value) = &function.return_value {
        let mut value = value.clone();
        crate::visit::visit_expression(&mut value, &mut visit)?;
    }
    drop(visit);
    match found {
        Some(diagnostic) => Err(diagnostic),
        None => Ok(()),
    }
}

fn parameter_environment(function: &Function) -> HashMap<String, TypeName> {
    function
        .parameters
        .iter()
        .map(|parameter| (parameter.name.clone(), parameter.ty.clone()))
        .collect()
}

fn resolve_template<'a>(
    name: &str,
    caller: &Function,
    templates: &'a HashMap<String, Function>,
) -> Option<&'a Function> {
    if let Some(function) = templates.get(name) {
        return Some(function);
    }
    let local_name = if caller.module_path.is_empty() {
        caller.name.as_str()
    } else {
        caller
            .name
            .strip_prefix(&format!("{}::", caller.module_path))?
    };
    if let Some((namespace, _)) = local_name.rsplit_once("::")
        && let Some(function) = templates.get(&format!("{namespace}::{name}"))
    {
        return Some(function);
    }
    if !caller.module_path.is_empty()
        && let Some(function) = templates.get(&format!("{}::{name}", caller.module_path))
    {
        return Some(function);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn specialize_call(
    name: &mut String,
    explicit_type_arguments: &[TypeName],
    arguments: &[Expression],
    span: Span,
    environment: &HashMap<String, TypeName>,
    caller: &Function,
    templates: &HashMap<String, Function>,
    functions: &HashMap<String, Function>,
    enum_definitions: &[EnumDef],
    specializations: &mut HashMap<String, String>,
    pending: &mut Vec<Function>,
) -> Result<(), Diagnostic> {
    let Some(template) = resolve_template(name, caller, templates) else {
        if !explicit_type_arguments.is_empty() && !is_comptime_intrinsic(name) {
            return Err(Diagnostic {
                code: "R0264",
                message: format!("`{name}` is not a generic function"),
                span,
                help: None,
            });
        }
        return Ok(());
    };
    if !explicit_type_arguments.is_empty()
        && explicit_type_arguments.len() != template.type_parameters.len()
    {
        return Err(Diagnostic {
            code: "R0264",
            message: format!(
                "generic function `{}` expects {} type argument(s), found {}",
                template.name,
                template.type_parameters.len(),
                explicit_type_arguments.len()
            ),
            span,
            help: None,
        });
    }
    let mut substitutions = HashMap::new();
    for (parameter, ty) in template.type_parameters.iter().zip(explicit_type_arguments) {
        substitutions.insert(parameter.clone(), ty.clone());
    }
    for (parameter, argument) in template.parameters.iter().zip(arguments) {
        // An untyped literal takes the type that an explicit type argument
        // already fixed; semantic analysis converts it to the parameter type.
        if let TypeName::Parameter(name, _) = &parameter.ty
            && substitutions.contains_key(name)
            && is_untyped_literal(argument)
        {
            continue;
        }
        // A parameter whose type does not mention a type parameter needs no inference.
        if is_fixed_type(&parameter.ty) {
            continue;
        }
        let Some(actual) = infer_type(
            argument,
            environment,
            functions,
            templates,
            enum_definitions,
            caller,
        ) else {
            return Err(Diagnostic {
                code: "R0262",
                message: format!(
                    "cannot infer generic type arguments for `{}`",
                    template.name
                ),
                span,
                help: Some("pass an expression with a known concrete type".into()),
            });
        };
        let mut argument_substitutions = HashMap::new();
        unify(
            &parameter.ty,
            &actual,
            &mut argument_substitutions,
            enum_definitions,
            span,
        )?;
        merge_substitutions(&mut substitutions, argument_substitutions, span)?;
    }
    // Generic Option/Result return signatures get their own parser-created
    // enum placeholders. Match those layouts to the concrete specialization
    // selected by the inferred payload types before substituting the function.
    for definition in enum_definitions {
        if !enum_has_type_parameters(definition)
            || !(definition.name.starts_with("$RynOption#")
                || definition.name.starts_with("$RynResult#"))
        {
            continue;
        }
        let mut specialized = definition.clone();
        for variant in &mut specialized.variants {
            for field in &mut variant.fields {
                substitute_type(field, &substitutions);
            }
        }
        if enum_has_type_parameters(&specialized) {
            continue;
        }
        let family = if definition.name.starts_with("$RynOption#") {
            "$RynOption#"
        } else {
            "$RynResult#"
        };
        let created = NEW_ENUMS.with(|enums| enums.borrow().clone());
        if let Some(concrete) = enum_definitions.iter().chain(&created).find(|candidate| {
            candidate.name != definition.name
                && candidate.name.starts_with(family)
                && candidate.variants.len() == specialized.variants.len()
                && candidate
                    .variants
                    .iter()
                    .zip(&specialized.variants)
                    .all(|(left, right)| {
                        left.name == right.name
                            && left.fields.len() == right.fields.len()
                            && left
                                .fields
                                .iter()
                                .zip(&right.fields)
                                .all(|(left, right)| type_key(left) == type_key(right))
                    })
        }) {
            substitutions.insert(
                definition.name.clone(),
                TypeName::Named(concrete.name.clone(), definition.span),
            );
        }
    }
    // A generic structure written with type parameters stands for the concrete instance whose
    // fields equal its fields under the inferred substitution.
    let placeholders: Vec<StructDef> = STRUCTS.with(|structs| {
        structs
            .borrow()
            .iter()
            .filter(|definition| definition.name.starts_with(STRUCT_PLACEHOLDER))
            .cloned()
            .collect()
    });
    for placeholder in placeholders {
        let mut fields = placeholder.fields.clone();
        for field in &mut fields {
            substitute_type(&mut field.ty, &substitutions);
        }
        let concrete = STRUCTS.with(|structs| {
            structs
                .borrow()
                .iter()
                .find(|candidate| {
                    candidate.name.starts_with(STRUCT_INSTANCE)
                        && candidate.fields.len() == fields.len()
                        && candidate.fields.iter().zip(&fields).all(|(left, right)| {
                            left.name == right.name && type_key(&left.ty) == type_key(&right.ty)
                        })
                })
                .map(|candidate| candidate.name.clone())
        });
        if let Some(concrete) = concrete {
            substitutions.insert(
                placeholder.name.clone(),
                TypeName::Named(concrete, placeholder.span),
            );
        }
    }
    // A tuple written with type parameters stands for the tuple of its element types under
    // the inferred substitution.
    let tuple_templates: Vec<StructDef> = STRUCTS.with(|structs| {
        structs
            .borrow()
            .iter()
            .filter(|definition| is_tuple_template(definition))
            .cloned()
            .collect()
    });
    for template_tuple in tuple_templates {
        let mut fields = template_tuple
            .fields
            .iter()
            .map(|field| {
                let mut ty = field.ty.clone();
                substitute_type(&mut ty, &substitutions);
                ty
            })
            .collect::<Vec<_>>();
        if fields.iter().any(type_has_parameter) {
            continue;
        }
        let concrete = tuple_type(std::mem::take(&mut fields), template_tuple.span);
        substitutions.insert(template_tuple.name.clone(), concrete);
    }
    for parameter in &template.type_parameters {
        if !substitutions.contains_key(parameter) {
            return Err(Diagnostic {
                code: "R0262",
                message: format!(
                    "cannot infer generic parameter `{parameter}` for `{}`",
                    template.name
                ),
                span,
                help: Some(
                    "use the parameter in an argument type so its type can be inferred".into(),
                ),
            });
        }
    }
    let concrete_bounds = template
        .type_parameter_bounds
        .iter()
        .filter_map(|(parameter, bounds)| {
            let concrete = substitutions.get(parameter)?;
            Some((type_key(concrete), bounds.clone()))
        })
        .collect::<Vec<_>>();
    let type_key = template
        .type_parameters
        .iter()
        .map(|parameter| {
            substitutions
                .get(parameter)
                .map(type_key)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(",");
    let specialization_key = format!("{}<{type_key}>", template.name);
    let specialized_name = if let Some(name) = specializations.get(&specialization_key) {
        name.clone()
    } else {
        if specializations.len() >= MAX_SPECIALIZATIONS {
            return Err(Diagnostic {
                code: "R0261",
                message: "generic specialization limit exceeded".into(),
                span,
                help: Some(
                    "reduce recursively growing type arguments or split the generic call graph"
                        .into(),
                ),
            });
        }
        let unique = specializations.len();
        let specialized_name = format!("{}$RynGeneric#{unique}", template.name);
        specializations.insert(specialization_key, specialized_name.clone());
        let mut function = template.clone();
        function.name.clone_from(&specialized_name);
        function.type_parameters.clear();
        // Bounds follow the substitution: the key becomes the concrete type name
        // so semantic analysis can check conformance on the specialization.
        function.type_parameter_bounds = concrete_bounds.clone();
        for parameter in &mut function.parameters {
            substitute_type(&mut parameter.ty, &substitutions);
        }
        if let Some(return_type) = &mut function.return_type {
            substitute_type(return_type, &substitutions);
        }
        substitute_statements(&mut function.body, &substitutions);
        if let Some(value) = &mut function.return_value {
            substitute_expression(value, &substitutions);
        }
        pending.push(function);
        specialized_name
    };
    *name = specialized_name;
    Ok(())
}

/// Compile-time metadata functions take explicit type arguments but are not generic functions.
pub fn is_comptime_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "field_count" | "variant_count" | "type_name" | "has_field"
    )
}

/// Whether a parameter type is concrete: it cannot mention a type parameter.
fn is_fixed_type(ty: &TypeName) -> bool {
    match ty {
        TypeName::I8
        | TypeName::I16
        | TypeName::I32
        | TypeName::I64
        | TypeName::U8
        | TypeName::U16
        | TypeName::U32
        | TypeName::U64
        | TypeName::F32
        | TypeName::F64
        | TypeName::Str
        | TypeName::OwnedString
        | TypeName::Char
        | TypeName::Bool => true,
        TypeName::Named(name, _) => !name.contains('<') && !name.contains('$'),
        TypeName::Reference(inner, _, _) | TypeName::RawPointer(inner, _) => is_fixed_type(inner),
        _ => false,
    }
}

fn is_untyped_literal(expression: &Expression) -> bool {
    match expression {
        Expression::Integer(..) | Expression::Float(..) => true,
        Expression::Negate(inner, _) => is_untyped_literal(inner),
        _ => false,
    }
}

fn unify(
    pattern: &TypeName,
    actual: &TypeName,
    substitutions: &mut HashMap<String, TypeName>,
    enum_definitions: &[EnumDef],
    span: Span,
) -> Result<(), Diagnostic> {
    match (pattern, actual) {
        (TypeName::Parameter(parameter, _), actual) => {
            if let Some(previous) = substitutions.get(parameter) {
                if type_key(previous) != type_key(actual) {
                    return Err(Diagnostic {
                        code: "R0263",
                        message: format!("generic parameter `{parameter}` has conflicting types"),
                        span,
                        help: Some(format!(
                            "this call uses both `{}` and `{}` for `{parameter}`",
                            display_key(previous),
                            display_key(actual)
                        )),
                    });
                }
            } else {
                substitutions.insert(parameter.clone(), actual.clone());
            }
        }
        (TypeName::Vec(left, _), TypeName::Vec(right, _))
        | (TypeName::Slice(left, _), TypeName::Slice(right, _)) => {
            unify(left, right, substitutions, enum_definitions, span)?;
        }
        (TypeName::Slice(left, _), TypeName::Array(right, _, _)) => {
            unify(left, right, substitutions, enum_definitions, span)?;
        }
        (TypeName::Array(left, left_len, _), TypeName::Array(right, right_len, _))
            if left_len == right_len =>
        {
            unify(left, right, substitutions, enum_definitions, span)?;
        }
        (TypeName::Map(left_key, left_value, _), TypeName::Map(right_key, right_value, _)) => {
            unify(left_key, right_key, substitutions, enum_definitions, span)?;
            unify(
                left_value,
                right_value,
                substitutions,
                enum_definitions,
                span,
            )?;
        }
        (
            TypeName::FunctionPointer(left_parameters, left_result, left_c_abi, _),
            TypeName::FunctionPointer(right_parameters, right_result, right_c_abi, _),
        ) if left_parameters.len() == right_parameters.len() && left_c_abi == right_c_abi => {
            for (left, right) in left_parameters.iter().zip(right_parameters) {
                unify(left, right, substitutions, enum_definitions, span)?;
            }
            match (left_result, right_result) {
                (Some(left), Some(right)) => {
                    unify(left, right, substitutions, enum_definitions, span)?;
                }
                (None, None) => {}
                _ => return Err(generic_type_mismatch(pattern, actual, span)),
            }
        }
        (TypeName::Named(pattern_name, _), TypeName::Named(actual_name, _))
            if pattern_name != actual_name
                && pattern_name.starts_with(STRUCT_PLACEHOLDER)
                && actual_name.starts_with(STRUCT_INSTANCE) =>
        {
            // `Stack<T>` against a concrete `Stack<i32>`: unify the fields.
            let (Some(pattern_struct), Some(actual_struct)) =
                (find_struct(pattern_name), find_struct(actual_name))
            else {
                return Err(generic_type_mismatch(pattern, actual, span));
            };
            if pattern_struct.fields.len() != actual_struct.fields.len()
                || pattern_struct
                    .fields
                    .iter()
                    .zip(&actual_struct.fields)
                    .any(|(left, right)| left.name != right.name)
            {
                return Err(generic_type_mismatch(pattern, actual, span));
            }
            for (left, right) in pattern_struct.fields.iter().zip(&actual_struct.fields) {
                unify(&left.ty, &right.ty, substitutions, enum_definitions, span)?;
            }
        }
        (TypeName::Named(pattern_name, _), TypeName::Named(actual_name, _))
            if pattern_name != actual_name
                && pattern_name.starts_with(TUPLE_PREFIX)
                && actual_name.starts_with(TUPLE_PREFIX) =>
        {
            // `(A, B)` against a concrete tuple: unify the element types.
            let (Some(pattern_tuple), Some(actual_tuple)) =
                (find_struct(pattern_name), find_struct(actual_name))
            else {
                return Err(generic_type_mismatch(pattern, actual, span));
            };
            if pattern_tuple.fields.len() != actual_tuple.fields.len() {
                return Err(generic_type_mismatch(pattern, actual, span));
            }
            for (left, right) in pattern_tuple.fields.iter().zip(&actual_tuple.fields) {
                unify(&left.ty, &right.ty, substitutions, enum_definitions, span)?;
            }
        }
        (
            TypeName::Named(pattern_name, pattern_span),
            TypeName::Named(actual_name, actual_span),
        ) if pattern_name != actual_name => {
            let pattern_enum = find_enum_definition(pattern_name, enum_definitions);
            let actual_enum = find_enum_definition(actual_name, enum_definitions);
            let same_family = |left: &str, right: &str| {
                [OPTION_PREFIX, "$RynResult#"]
                    .iter()
                    .any(|prefix| left.starts_with(prefix) && right.starts_with(prefix))
                    || enum_placeholder_matches(left, right)
            };
            if let (Some(pattern_enum), Some(actual_enum)) = (pattern_enum, actual_enum)
                && same_family(&pattern_enum.name, &actual_enum.name)
                && pattern_enum.variants.len() == actual_enum.variants.len()
            {
                for (pattern_variant, actual_variant) in
                    pattern_enum.variants.iter().zip(&actual_enum.variants)
                {
                    if pattern_variant.name != actual_variant.name
                        || pattern_variant.fields.len() != actual_variant.fields.len()
                    {
                        return Err(generic_type_mismatch(pattern, actual, span));
                    }
                    for (pattern_field, actual_field) in
                        pattern_variant.fields.iter().zip(&actual_variant.fields)
                    {
                        unify(
                            pattern_field,
                            actual_field,
                            substitutions,
                            enum_definitions,
                            span,
                        )?;
                    }
                }
                substitutions.insert(
                    pattern_name.clone(),
                    TypeName::Named(actual_name.clone(), *actual_span),
                );
            } else {
                return Err(generic_type_mismatch(pattern, actual, *pattern_span));
            }
        }
        _ if type_key(pattern) == type_key(actual) => {}
        _ => {
            return Err(generic_type_mismatch(pattern, actual, span));
        }
    }
    Ok(())
}

fn merge_substitutions(
    substitutions: &mut HashMap<String, TypeName>,
    new_substitutions: HashMap<String, TypeName>,
    span: Span,
) -> Result<(), Diagnostic> {
    for (parameter, actual) in new_substitutions {
        if let Some(previous) = substitutions.get(&parameter) {
            if type_key(previous) != type_key(&actual) {
                return Err(Diagnostic {
                    code: "R0263",
                    message: format!("generic parameter `{parameter}` has conflicting types"),
                    span,
                    help: Some(format!(
                        "this call uses both `{}` and `{}` for `{parameter}`",
                        display_key(previous),
                        display_key(&actual)
                    )),
                });
            }
        } else {
            substitutions.insert(parameter, actual);
        }
    }
    Ok(())
}
fn generic_type_mismatch(pattern: &TypeName, actual: &TypeName, span: Span) -> Diagnostic {
    Diagnostic {
        code: "R0263",
        message: format!(
            "generic argument has type `{}` but the parameter pattern is `{}`",
            display_key(actual),
            display_key(pattern)
        ),
        span,
        help: Some("pass a value whose type matches the generic function parameter".into()),
    }
}

/// A type as a diagnostic shows it: tuples and enum instances appear in their source forms
/// instead of their internal names.
fn display_key(ty: &TypeName) -> String {
    match ty {
        TypeName::Named(name, _) => display_name(name),
        TypeName::Parameter(name, _) => name.clone(),
        TypeName::Vec(element, _) => format!("Vec<{}>", display_key(element)),
        TypeName::Set(element, _) => format!("Set<{}>", display_key(element)),
        TypeName::Map(key, value, _) => format!("Map<{},{}>", display_key(key), display_key(value)),
        TypeName::Array(element, length, _) => format!("[{};{length}]", display_key(element)),
        TypeName::Slice(element, _) => format!("&[{}]", display_key(element)),
        TypeName::Reference(element, mutable, _) => {
            format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                display_key(element)
            )
        }
        TypeName::RawPointer(element, _) => format!("*{}", display_key(element)),
        TypeName::FunctionPointer(parameters, result, extern_c, _) => {
            let prefix = if *extern_c { "extern \"C\" " } else { "" };
            let parameters = parameters
                .iter()
                .map(display_key)
                .collect::<Vec<_>>()
                .join(",");
            match result {
                Some(result) => format!("{prefix}fun({parameters})->{}", display_key(result)),
                None => format!("{prefix}fun({parameters})"),
            }
        }
        _ => type_key(ty),
    }
}

fn display_name(name: &str) -> String {
    if name.starts_with(TUPLE_PREFIX)
        && let Some(definition) = find_struct(name)
    {
        let fields = definition
            .fields
            .iter()
            .map(|field| display_key(&field.ty))
            .collect::<Vec<_>>();
        return format!("({})", fields.join(", "));
    }
    if name.starts_with(OPTION_PREFIX) {
        return "Option".into();
    }
    if name.starts_with("$RynResult#") {
        return "Result".into();
    }
    for prefix in [ENUM_PLACEHOLDER, ENUM_INSTANCE] {
        if let Some((template, _)) = name
            .strip_prefix(prefix)
            .and_then(|rest| rest.rsplit_once('#'))
        {
            return template.to_owned();
        }
    }
    name.to_owned()
}

fn type_key(ty: &TypeName) -> String {
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
        TypeName::Named(name, _) => name.clone(),
        TypeName::Parameter(name, _) => format!("${name}"),
        TypeName::Vec(element, _) => format!("Vec<{}>", type_key(element)),
        TypeName::Set(element, _) => format!("Set<{}>", type_key(element)),
        TypeName::Map(key, value, _) => format!("Map<{},{}>", type_key(key), type_key(value)),
        TypeName::Array(element, length, _) => format!("[{};{length}]", type_key(element)),
        TypeName::Slice(element, _) => format!("&[{}]", type_key(element)),
        TypeName::Reference(element, mutable, _) => format!(
            "&{}{}",
            if *mutable { "mut " } else { "" },
            type_key(element)
        ),
        TypeName::RawPointer(element, _) => format!("*{}", type_key(element)),
        TypeName::FunctionPointer(parameters, result, extern_c, _) => {
            let prefix = if *extern_c { "extern \"C\" " } else { "" };
            let parameters = parameters
                .iter()
                .map(type_key)
                .collect::<Vec<_>>()
                .join(",");
            match result {
                Some(result) => format!("{prefix}fun({parameters})->{}", type_key(result)),
                None => format!("{prefix}fun({parameters})"),
            }
        }
    }
}

fn substitute_type(ty: &mut TypeName, substitutions: &HashMap<String, TypeName>) {
    match ty {
        TypeName::Named(name, _) => {
            if let Some(replacement) = substitutions.get(name) {
                *ty = replacement.clone();
            }
        }
        TypeName::Parameter(name, _) => {
            if let Some(replacement) = substitutions.get(name) {
                *ty = replacement.clone();
            }
        }
        TypeName::Vec(element, _)
        | TypeName::Set(element, _)
        | TypeName::Slice(element, _)
        | TypeName::Reference(element, _, _)
        | TypeName::RawPointer(element, _) => substitute_type(element, substitutions),
        TypeName::Map(key, value, _) => {
            substitute_type(key, substitutions);
            substitute_type(value, substitutions);
        }
        TypeName::Array(element, _, _) => substitute_type(element, substitutions),
        TypeName::FunctionPointer(parameters, result, _, _) => {
            for parameter in parameters {
                substitute_type(parameter, substitutions);
            }
            if let Some(result) = result {
                substitute_type(result, substitutions);
            }
        }
        _ => {}
    }
}

fn substitute_statements(statements: &mut [Statement], substitutions: &HashMap<String, TypeName>) {
    for statement in statements {
        match statement {
            Statement::Let {
                annotation, value, ..
            } => {
                if let Some(annotation) = annotation {
                    substitute_type(annotation, substitutions);
                }
                substitute_expression(value, substitutions);
            }
            Statement::Assign { value, .. }
            | Statement::CompoundAssign { value, .. }
            | Statement::Print(value, _) => substitute_expression(value, substitutions),
            Statement::IndexAssign { index, value, .. } => {
                substitute_expression(index, substitutions);
                substitute_expression(value, substitutions);
            }
            Statement::DereferenceAssign { pointer, value, .. } => {
                substitute_expression(pointer, substitutions);
                substitute_expression(value, substitutions);
            }
            Statement::FieldAssign { value, .. } => substitute_expression(value, substitutions),
            Statement::PrintTemplate(parts, _) => {
                for part in parts {
                    if let PrintPart::Value(value) = part {
                        substitute_expression(value, substitutions);
                    }
                }
            }
            Statement::Call {
                type_arguments,
                arguments,
                ..
            } => {
                for argument in type_arguments {
                    substitute_type(argument, substitutions);
                }
                for argument in arguments {
                    substitute_expression(argument, substitutions);
                }
            }
            Statement::MethodCall {
                value, arguments, ..
            } => {
                substitute_expression(value, substitutions);
                for argument in arguments {
                    substitute_expression(argument, substitutions);
                }
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                substitute_expression(condition, substitutions);
                substitute_statements(then_body, substitutions);
                substitute_statements(else_body, substitutions);
            }
            Statement::While {
                condition, body, ..
            } => {
                substitute_expression(condition, substitutions);
                substitute_statements(body, substitutions);
            }
            Statement::Defer { body, .. } => {
                substitute_statements(body, substitutions);
            }
            Statement::For {
                start, end, body, ..
            } => {
                substitute_expression(start, substitutions);
                substitute_expression(end, substitutions);
                substitute_statements(body, substitutions);
            }
            Statement::ForEach {
                collection, body, ..
            } => {
                substitute_expression(collection, substitutions);
                substitute_statements(body, substitutions);
            }
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    substitute_expression(value, substitutions);
                }
            }
            Statement::Break(_) | Statement::Continue(_) => {}
        }
    }
}

fn substitute_expression(expression: &mut Expression, substitutions: &HashMap<String, TypeName>) {
    match expression {
        Expression::Call {
            type_arguments,
            arguments,
            ..
        } => {
            for argument in type_arguments {
                substitute_type(argument, substitutions);
            }
            for argument in arguments {
                substitute_expression(argument, substitutions);
            }
        }
        Expression::MethodCall {
            value,
            type_arguments,
            arguments,
            ..
        } => {
            substitute_expression(value, substitutions);
            for argument in type_arguments {
                substitute_type(argument, substitutions);
            }
            for argument in arguments {
                substitute_expression(argument, substitutions);
            }
        }
        Expression::StructLiteral { name, fields, .. } => {
            if let Some(TypeName::Named(replacement, _)) = substitutions.get(name.as_str()) {
                name.clone_from(replacement);
            }
            for (_, value, _) in fields {
                substitute_expression(value, substitutions);
            }
        }
        Expression::Field { value, .. }
        | Expression::Negate(value, _)
        | Expression::Not(value, _)
        | Expression::BitNot(value, _)
        | Expression::Dereference(value, _)
        | Expression::Propagate(value, _) => substitute_expression(value, substitutions),
        Expression::AddressOf { value, .. } => substitute_expression(value, substitutions),
        Expression::If {
            condition,
            then_value,
            else_value,
            ..
        } => {
            substitute_expression(condition, substitutions);
            substitute_expression(then_value, substitutions);
            substitute_expression(else_value, substitutions);
        }
        Expression::Binary { left, right, .. } => {
            substitute_expression(left, substitutions);
            substitute_expression(right, substitutions);
        }
        Expression::Range { start, end, .. } => {
            substitute_expression(start, substitutions);
            substitute_expression(end, substitutions);
        }
        Expression::Cast(value, ty, _) => {
            substitute_expression(value, substitutions);
            substitute_type(ty, substitutions);
        }
        Expression::LayoutOf { ty, .. } => substitute_type(ty, substitutions),
        Expression::VecConstructor { element, .. } => substitute_type(element, substitutions),
        Expression::MapConstructor { key, value, .. } => {
            substitute_type(key, substitutions);
            substitute_type(value, substitutions);
        }
        Expression::SetConstructor { element, .. } => substitute_type(element, substitutions),
        Expression::ArrayLiteral(values, _) => {
            for value in values {
                substitute_expression(value, substitutions);
            }
        }
        Expression::ArrayRepeat { value, .. } => substitute_expression(value, substitutions),
        Expression::Tuple(values, _) => {
            for value in values {
                substitute_expression(value, substitutions);
            }
        }
        Expression::Index { value, index, .. } => {
            substitute_expression(value, substitutions);
            substitute_expression(index, substitutions);
        }
        Expression::EnumConstruct {
            enum_name,
            arguments,
            ..
        } => {
            if let Some(TypeName::Named(replacement, _)) = substitutions.get(enum_name) {
                *enum_name = replacement.clone();
            }
            for argument in arguments {
                substitute_expression(argument, substitutions);
            }
        }
        Expression::Choose { value, arms, .. } => {
            substitute_expression(value, substitutions);
            for arm in arms {
                substitute_expression(&mut arm.body, substitutions);
            }
        }
        Expression::Integer(..)
        | Expression::Float(..)
        | Expression::String(..)
        | Expression::Character(..)
        | Expression::Boolean(..)
        | Expression::Name(..) => {}
    }
}

// These references are the shared state of one AST rewrite pass; grouping them
// would add an otherwise opaque context object to every recursive call.
#[allow(clippy::too_many_arguments)]
fn rewrite_statements(
    statements: &mut [Statement],
    environment: &mut HashMap<String, TypeName>,
    caller: &Function,
    templates: &HashMap<String, Function>,
    functions: &HashMap<String, Function>,
    enum_definitions: &[EnumDef],
    specializations: &mut HashMap<String, String>,
    pending: &mut Vec<Function>,
) -> Result<(), Diagnostic> {
    for statement in statements {
        match statement {
            Statement::Let {
                name,
                annotation,
                value,
                ..
            } => {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                if let Some(ty) = annotation {
                    environment.insert(name.clone(), ty.clone());
                } else if let Some(ty) = infer_type(
                    value,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                ) {
                    environment.insert(name.clone(), ty);
                }
            }
            Statement::Assign { value, .. }
            | Statement::CompoundAssign { value, .. }
            | Statement::Print(value, _) => {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::IndexAssign { index, value, .. } => {
                rewrite_expression(
                    index,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::DereferenceAssign { pointer, value, .. } => {
                rewrite_expression(
                    pointer,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::FieldAssign { value, .. } => {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::PrintTemplate(parts, _) => {
                for part in parts {
                    if let PrintPart::Value(value) = part {
                        rewrite_expression(
                            value,
                            environment,
                            caller,
                            templates,
                            functions,
                            enum_definitions,
                            specializations,
                            pending,
                        )?;
                    }
                }
            }
            Statement::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => {
                for ty in type_arguments.iter_mut() {
                    substitute_type(ty, environment);
                }
                for argument in arguments.iter_mut() {
                    rewrite_expression(
                        argument,
                        environment,
                        caller,
                        templates,
                        functions,
                        enum_definitions,
                        specializations,
                        pending,
                    )?;
                }
                specialize_call(
                    name,
                    type_arguments,
                    arguments,
                    *span,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                if !is_comptime_intrinsic(name) {
                    type_arguments.clear();
                }
            }
            Statement::MethodCall {
                value, arguments, ..
            } => {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                for argument in arguments {
                    rewrite_expression(
                        argument,
                        environment,
                        caller,
                        templates,
                        functions,
                        enum_definitions,
                        specializations,
                        pending,
                    )?;
                }
            }
            Statement::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                rewrite_expression(
                    condition,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                let mut then_environment = environment.clone();
                let mut else_environment = environment.clone();
                rewrite_statements(
                    then_body,
                    &mut then_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                rewrite_statements(
                    else_body,
                    &mut else_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::While {
                condition, body, ..
            } => {
                rewrite_expression(
                    condition,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                let mut loop_environment = environment.clone();
                rewrite_statements(
                    body,
                    &mut loop_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::Defer { body, .. } => {
                let mut block_environment = environment.clone();
                rewrite_statements(
                    body,
                    &mut block_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::For {
                name,
                start,
                end,
                body,
                ..
            } => {
                rewrite_expression(
                    start,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                rewrite_expression(
                    end,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                let mut loop_environment = environment.clone();
                if let Some(ty) = infer_type(
                    start,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                ) {
                    loop_environment.insert(name.clone(), ty);
                }
                rewrite_statements(
                    body,
                    &mut loop_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::ForEach {
                name,
                collection,
                body,
                ..
            } => {
                rewrite_expression(
                    collection,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
                let mut loop_environment = environment.clone();
                if let Some(ty) = infer_type(
                    collection,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )
                .and_then(collection_element)
                {
                    loop_environment.insert(name.clone(), ty);
                }
                rewrite_statements(
                    body,
                    &mut loop_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    rewrite_expression(
                        value,
                        environment,
                        caller,
                        templates,
                        functions,
                        enum_definitions,
                        specializations,
                        pending,
                    )?;
                }
            }
            Statement::Break(_) | Statement::Continue(_) => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rewrite_expression(
    expression: &mut Expression,
    environment: &HashMap<String, TypeName>,
    caller: &Function,
    templates: &HashMap<String, Function>,
    functions: &HashMap<String, Function>,
    enum_definitions: &[EnumDef],
    specializations: &mut HashMap<String, String>,
    pending: &mut Vec<Function>,
) -> Result<(), Diagnostic> {
    match expression {
        Expression::Call {
            name,
            type_arguments,
            arguments,
            span,
        } => {
            for ty in type_arguments.iter_mut() {
                substitute_type(ty, environment);
            }
            // Types are inferred from the arguments as written: a nested call
            // to a generic function is rewritten to a specialization that
            // `infer_type` cannot see yet, but its template can.
            let written_arguments = arguments.clone();
            for argument in arguments.iter_mut() {
                rewrite_expression(
                    argument,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
            specialize_call(
                name,
                type_arguments,
                &written_arguments,
                *span,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            if !is_comptime_intrinsic(name) {
                type_arguments.clear();
            }
        }
        Expression::MethodCall {
            value,
            name,
            type_arguments,
            arguments,
            span,
        } => {
            let receiver_type = infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            );
            let helper_module = receiver_type.as_ref().and_then(|ty| {
                let TypeName::Named(enum_name, _) = ty else {
                    return None;
                };
                let family = if enum_name.starts_with("$RynOption#") {
                    "Option"
                } else if enum_name.starts_with("$RynResult#") {
                    "Result"
                } else {
                    return None;
                };
                match (family, name.as_str()) {
                    ("Option", "map") => Some(("option", "map")),
                    ("Result", "map") => Some(("result", "map")),
                    ("Result", "map_err") => Some(("result", "map_err")),
                    _ => None,
                }
            });
            if let Some((module, function)) = helper_module {
                let helper_name = format!("{module}::{function}");
                if resolve_template(&helper_name, caller, templates).is_some()
                    && arguments.len() == 1
                {
                    let receiver =
                        std::mem::replace(value, Box::new(Expression::Boolean(false, *span)));
                    let mapper = arguments.remove(0);
                    *expression = Expression::Call {
                        name: helper_name,
                        type_arguments: Vec::new(),
                        arguments: vec![*receiver, mapper],
                        span: *span,
                    };
                    return rewrite_expression(
                        expression,
                        environment,
                        caller,
                        templates,
                        functions,
                        enum_definitions,
                        specializations,
                        pending,
                    );
                }
            }
            for argument in type_arguments.iter_mut() {
                substitute_type(argument, environment);
            }
            rewrite_expression(
                value,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            for argument in arguments {
                rewrite_expression(
                    argument,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::StructLiteral { fields, .. } => {
            for (_, value, _) in fields {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::Field { value, .. }
        | Expression::Negate(value, _)
        | Expression::Not(value, _)
        | Expression::BitNot(value, _)
        | Expression::Dereference(value, _)
        | Expression::Propagate(value, _) => rewrite_expression(
            value,
            environment,
            caller,
            templates,
            functions,
            enum_definitions,
            specializations,
            pending,
        )?,
        Expression::AddressOf { value, .. } => rewrite_expression(
            value,
            environment,
            caller,
            templates,
            functions,
            enum_definitions,
            specializations,
            pending,
        )?,
        Expression::If {
            condition,
            then_value,
            else_value,
            ..
        } => {
            rewrite_expression(
                condition,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            rewrite_expression(
                then_value,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            rewrite_expression(
                else_value,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
        }
        Expression::Binary { left, right, .. } => {
            rewrite_expression(
                left,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            rewrite_expression(
                right,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
        }
        Expression::Range { start, end, .. } => {
            rewrite_expression(
                start,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            rewrite_expression(
                end,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
        }
        Expression::Cast(value, _, _) => rewrite_expression(
            value,
            environment,
            caller,
            templates,
            functions,
            enum_definitions,
            specializations,
            pending,
        )?,
        Expression::LayoutOf { .. } => {}
        Expression::ArrayLiteral(values, _) => {
            for value in values {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::ArrayRepeat { value, .. } => rewrite_expression(
            value,
            environment,
            caller,
            templates,
            functions,
            enum_definitions,
            specializations,
            pending,
        )?,
        Expression::Tuple(values, _) => {
            for value in values {
                rewrite_expression(
                    value,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::Index { value, index, .. } => {
            rewrite_expression(
                value,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            rewrite_expression(
                index,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
        }
        Expression::EnumConstruct { arguments, .. } => {
            for argument in arguments {
                rewrite_expression(
                    argument,
                    environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::Choose { value, arms, .. } => {
            let value_type = infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            );
            rewrite_expression(
                value,
                environment,
                caller,
                templates,
                functions,
                enum_definitions,
                specializations,
                pending,
            )?;
            for arm in arms {
                let mut arm_environment = environment.clone();
                if let Some(TypeName::Named(enum_name, _)) = &value_type
                    && let Some(definition) = enum_definitions
                        .iter()
                        .find(|definition| definition.name == *enum_name)
                    && let Some(variant) = arm.variant.as_ref().and_then(|variant_name| {
                        definition
                            .variants
                            .iter()
                            .find(|variant| variant.name == *variant_name)
                    })
                {
                    for (binding, field_type) in arm.bindings.iter().zip(&variant.fields) {
                        arm_environment.insert(binding.clone(), field_type.clone());
                    }
                }
                rewrite_expression(
                    &mut arm.body,
                    &arm_environment,
                    caller,
                    templates,
                    functions,
                    enum_definitions,
                    specializations,
                    pending,
                )?;
            }
        }
        Expression::Integer(..)
        | Expression::Float(..)
        | Expression::String(..)
        | Expression::Character(..)
        | Expression::Boolean(..)
        | Expression::Name(..)
        | Expression::VecConstructor { .. }
        | Expression::MapConstructor { .. }
        | Expression::SetConstructor { .. } => {}
    }
    Ok(())
}

fn infer_type(
    expression: &Expression,
    environment: &HashMap<String, TypeName>,
    functions: &HashMap<String, Function>,
    templates: &HashMap<String, Function>,
    enum_definitions: &[EnumDef],
    caller: &Function,
) -> Option<TypeName> {
    match expression {
        Expression::Integer(_, _) => Some(TypeName::I64),
        Expression::Float(_, _) => Some(TypeName::F64),
        Expression::String(_, _) => Some(TypeName::Str),
        Expression::Character(_, _) => Some(TypeName::Char),
        Expression::Boolean(_, _) => Some(TypeName::Bool),
        Expression::Name(name, span) => environment.get(name).cloned().or_else(|| {
            // A plain function used as a value has a function-pointer type.
            let function = functions.get(name)?;
            if function.extern_c || !function.type_parameters.is_empty() {
                return None;
            }
            Some(TypeName::FunctionPointer(
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                function.return_type.clone().map(Box::new),
                false,
                *span,
            ))
        }),
        Expression::Cast(_, ty, _) => Some(ty.clone()),
        Expression::Range { .. } => None,
        Expression::LayoutOf { .. } => Some(TypeName::U64),
        Expression::VecConstructor { element, .. } => {
            Some(TypeName::Vec(Box::new(element.clone()), expression.span()))
        }
        Expression::MapConstructor { key, value, .. } => Some(TypeName::Map(
            Box::new(key.clone()),
            Box::new(value.clone()),
            expression.span(),
        )),
        Expression::SetConstructor { element, .. } => {
            Some(TypeName::Set(Box::new(element.clone()), expression.span()))
        }
        Expression::StructLiteral { name, .. } => {
            Some(TypeName::Named(name.clone(), expression.span()))
        }
        Expression::Call {
            name, arguments, ..
        } => {
            if name == "String" {
                return Some(TypeName::OwnedString);
            }
            // `Option::Some(value)` is parsed as a call; it takes its `Option<T>` from the payload.
            if name == "Option::Some" && arguments.len() == 1 {
                let payload = infer_type(
                    &arguments[0],
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )?;
                return Some(option_type(payload, expression.span(), enum_definitions));
            }
            // `Enum::Variant(payload)` of a generic user enum takes its type arguments from the
            // payload, through the enum's placeholder.
            if let Some((enum_name, variant)) = name.rsplit_once("::")
                && !matches!(enum_name, "Option" | "Result")
                && enum_definitions.iter().any(|definition| {
                    definition
                        .name
                        .starts_with(&format!("{ENUM_PLACEHOLDER}{enum_name}#"))
                        && definition
                            .variants
                            .iter()
                            .any(|candidate| candidate.name == variant)
                })
            {
                let mut payload = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    payload.push(infer_type(
                        argument,
                        environment,
                        functions,
                        templates,
                        enum_definitions,
                        caller,
                    )?);
                }
                return generic_enum_instance(
                    enum_name,
                    variant,
                    &payload,
                    expression.span(),
                    enum_definitions,
                );
            }
            if let Some(TypeName::FunctionPointer(_, result, _, _)) = environment.get(name) {
                return result.as_deref().cloned();
            }
            let function = functions
                .get(name)
                .or_else(|| resolve_template(name, caller, templates))?;
            let return_type = function.return_type.clone()?;
            if function.type_parameters.is_empty() {
                return Some(return_type);
            }
            let mut substitutions = HashMap::new();
            for (parameter, argument) in function.parameters.iter().zip(arguments) {
                let actual = infer_type(
                    argument,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )?;
                let mut call_substitutions = HashMap::new();
                unify(
                    &parameter.ty,
                    &actual,
                    &mut call_substitutions,
                    enum_definitions,
                    expression.span(),
                )
                .ok()?;
                merge_substitutions(&mut substitutions, call_substitutions, expression.span())
                    .ok()?;
            }
            let mut result = return_type;
            substitute_type(&mut result, &substitutions);
            Some(result)
        }
        Expression::MethodCall { value, name, .. } => {
            let value_type = infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )?;
            match (value_type, name.as_str()) {
                (TypeName::Vec(element, span), "as_slice" | "slice") => {
                    Some(TypeName::Slice(element, span))
                }
                (TypeName::Vec(_, _), "len" | "capacity") => Some(TypeName::U64),
                (TypeName::Vec(element, _), "take" | "extract" | "index") => Some(*element),
                (TypeName::OwnedString, "clone" | "concat" | "slice" | "slice_chars" | "trim") => {
                    Some(TypeName::OwnedString)
                }
                (TypeName::OwnedString, "len" | "char_count") => Some(TypeName::U64),
                (TypeName::OwnedString, "char_at") => Some(TypeName::Char),
                (TypeName::Slice(_, _), "len") => Some(TypeName::U64),
                _ => None,
            }
        }
        Expression::ArrayLiteral(values, span) => {
            let element = values.first().and_then(|value| {
                infer_type(
                    value,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )
            })?;
            Some(TypeName::Array(Box::new(element), values.len(), *span))
        }
        Expression::ArrayRepeat {
            value,
            length,
            span,
        } => {
            let element = infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )?;
            Some(TypeName::Array(
                Box::new(element),
                usize::try_from(*length).ok()?,
                *span,
            ))
        }
        Expression::Tuple(elements, _) if !elements.is_empty() => {
            let mut fields = Vec::with_capacity(elements.len());
            for element in elements {
                fields.push(infer_type(
                    element,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )?);
            }
            Some(tuple_type(fields, expression.span()))
        }
        Expression::Tuple(_, _) => None,
        Expression::Index { value, .. } => {
            match infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )? {
                TypeName::Array(element, _, _)
                | TypeName::Slice(element, _)
                | TypeName::Vec(element, _) => Some(*element),
                _ => None,
            }
        }
        Expression::Field { value, name, .. } => {
            let _ = (value, name, caller);
            None
        }
        Expression::Binary { op, left, .. } => {
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or
            ) {
                Some(TypeName::Bool)
            } else {
                infer_type(
                    left,
                    environment,
                    functions,
                    templates,
                    enum_definitions,
                    caller,
                )
            }
        }
        Expression::If { then_value, .. } => infer_type(
            then_value,
            environment,
            functions,
            templates,
            enum_definitions,
            caller,
        ),
        Expression::Choose { arms, .. } => arms.first().and_then(|arm| {
            infer_type(
                &arm.body,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )
        }),
        Expression::Negate(inner, _) | Expression::BitNot(inner, _) => infer_type(
            inner,
            environment,
            functions,
            templates,
            enum_definitions,
            caller,
        ),
        Expression::AddressOf { mutable, value, .. } => Some(TypeName::Reference(
            Box::new(infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )?),
            *mutable,
            expression.span(),
        )),
        Expression::Dereference(value, _) => {
            match infer_type(
                value,
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )? {
                TypeName::Reference(element, _, _) | TypeName::RawPointer(element, _) => {
                    Some(*element)
                }
                _ => None,
            }
        }
        // `Option::Some(value)` takes its `Option<T>` from the payload's type.
        Expression::EnumConstruct {
            enum_name,
            variant,
            arguments,
            span,
        } if enum_name == "Option" && variant == "Some" && arguments.len() == 1 => {
            let payload = infer_type(
                &arguments[0],
                environment,
                functions,
                templates,
                enum_definitions,
                caller,
            )?;
            Some(option_type(payload, *span, enum_definitions))
        }
        Expression::EnumConstruct { .. } | Expression::Not(_, _) | Expression::Propagate(_, _) => {
            None
        }
    }
}

fn collection_element(ty: TypeName) -> Option<TypeName> {
    match ty {
        TypeName::Vec(element, _)
        | TypeName::Slice(element, _)
        | TypeName::Array(element, _, _) => Some(*element),
        TypeName::Str | TypeName::OwnedString => Some(TypeName::Char),
        _ => None,
    }
}

fn enum_has_type_parameters(definition: &EnumDef) -> bool {
    definition
        .variants
        .iter()
        .flat_map(|variant| &variant.fields)
        .any(type_has_parameter)
}

fn type_has_parameter(ty: &TypeName) -> bool {
    match ty {
        TypeName::Parameter(..) => true,
        TypeName::Array(element, _, _)
        | TypeName::Slice(element, _)
        | TypeName::Vec(element, _)
        | TypeName::Reference(element, _, _)
        | TypeName::RawPointer(element, _) => type_has_parameter(element),
        TypeName::Map(key, value, _) => type_has_parameter(key) || type_has_parameter(value),
        _ => false,
    }
}
