//! Compiles the extended forms of `choose` patterns into plain `choose` arms
//! and `when` chains.
//!
//! The parsers accept, besides `Enum::Variant(a, b)` and `_`, literal patterns
//! (`0`, `-3`, `'x'`, `true`) and nested variant patterns
//! (`Outer::Inner(Inner::Leaf(n))`). They keep these in the existing arm
//! shape: a literal arm has the enum name `$lit` and the canonical literal as
//! its variant, and a payload position that is not a plain name holds the
//! canonical text of its pattern. This pass rewrites every such `choose` with
//! the classic pattern-matrix algorithm so semantic analysis only sees
//! variant and `_` arms, and it reports arms that can never be selected.

use crate::{
    ast::{BinaryOp, ChooseArm, Expression, Program},
    source::{Diagnostic, Span},
    visit::{children_mut, visit_program},
};

#[derive(Clone, Debug, PartialEq)]
enum Pat {
    Wild,
    Bind(String),
    /// A literal in canonical text form.
    Lit(String),
    Ctor {
        enum_name: String,
        variant: String,
        args: Vec<Pat>,
    },
    /// `(a, b)`: one pattern per tuple element.
    Tuple(Vec<Pat>),
    /// `Point { x: 1, y }`: patterns for some of a structure's fields.
    Fields(Vec<(String, Pat)>),
}

#[derive(Clone)]
struct Row {
    pats: Vec<Pat>,
    binds: Vec<(String, Expression)>,
    body: Expression,
    index: usize,
    span: Span,
}

struct Compiler {
    used: Vec<bool>,
    fresh: usize,
}

fn error(message: impl Into<String>, span: Span) -> Diagnostic {
    Diagnostic {
        code: "R0235",
        message: message.into(),
        span,
        help: None,
    }
}

fn is_plain_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
        && !text.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        && text != "true"
        && text != "false"
}

/// Rewrites every `choose` that uses a literal or nested pattern.
pub fn desugar(program: &mut Program) -> Result<(), Diagnostic> {
    let mut fresh = 0;
    visit_program(program, &mut |expression| {
        let Expression::Choose { arms, .. } = expression else {
            return Ok(());
        };
        let extended = arms.iter().any(|arm| {
            matches!(arm.enum_name.as_deref(), Some("$lit" | "$pat"))
                || arm.bindings.iter().any(|binding| !is_plain_name(binding))
        });
        if !extended {
            return Ok(());
        }
        let Expression::Choose { value, arms, span } =
            std::mem::replace(expression, Expression::Boolean(false, Span::default()))
        else {
            unreachable!("matched a choose above");
        };
        *expression = compile_choose(*value, arms, span, &mut fresh)?;
        Ok(())
    })
}

fn parse_pattern(text: &str, span: Span) -> Result<Pat, Diagnostic> {
    let mut parser = PatternParser {
        text: text.as_bytes(),
        at: 0,
        span,
    };
    let pattern = parser.pattern()?;
    if parser.at != parser.text.len() {
        return Err(error("malformed pattern", span));
    }
    Ok(pattern)
}

struct PatternParser<'a> {
    text: &'a [u8],
    at: usize,
    span: Span,
}

impl PatternParser<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte == b'_' || byte == b'$' || byte.is_ascii_alphanumeric())
        {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.text[start..self.at]).into_owned()
    }

    fn pattern(&mut self) -> Result<Pat, Diagnostic> {
        match self.peek() {
            Some(b'(') => {
                self.at += 1;
                let mut elements = Vec::new();
                if self.peek() != Some(b')') {
                    loop {
                        elements.push(self.pattern()?);
                        if self.peek() == Some(b',') {
                            self.at += 1;
                        } else {
                            break;
                        }
                    }
                }
                if self.peek() != Some(b')') {
                    return Err(error("malformed pattern", self.span));
                }
                self.at += 1;
                Ok(Pat::Tuple(elements))
            }
            Some(b'"') => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|byte| byte != b'"') {
                    self.at += 1;
                }
                self.at += 1;
                Ok(Pat::Lit(
                    String::from_utf8_lossy(&self.text[start..self.at.min(self.text.len())])
                        .into_owned(),
                ))
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.at += 1;
                }
                Ok(Pat::Lit(
                    String::from_utf8_lossy(&self.text[start..self.at]).into_owned(),
                ))
            }
            Some(b'\'') => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|byte| byte != b'\'') {
                    self.at += 1;
                }
                self.at += 1;
                Ok(Pat::Lit(
                    String::from_utf8_lossy(&self.text[start..self.at.min(self.text.len())])
                        .into_owned(),
                ))
            }
            _ => {
                let first = self.word();
                if first.is_empty() {
                    return Err(error("malformed pattern", self.span));
                }
                if first == "true" || first == "false" {
                    return Ok(Pat::Lit(first));
                }
                if self.peek() == Some(b'{') {
                    self.at += 1;
                    let mut fields = Vec::new();
                    if self.peek() != Some(b'}') {
                        loop {
                            let name = self.word();
                            if name.is_empty() {
                                return Err(error("malformed pattern", self.span));
                            }
                            if self.peek() == Some(b':') {
                                self.at += 1;
                                fields.push((name, self.pattern()?));
                            } else {
                                fields.push((name.clone(), Pat::Bind(name)));
                            }
                            if self.peek() == Some(b',') {
                                self.at += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    if self.peek() != Some(b'}') {
                        return Err(error("malformed pattern", self.span));
                    }
                    self.at += 1;
                    return Ok(Pat::Fields(fields));
                }
                let mut path = vec![first];
                while self.text[self.at..].starts_with(b"::") {
                    self.at += 2;
                    path.push(self.word());
                }
                let mut args = Vec::new();
                if self.peek() == Some(b'(') {
                    self.at += 1;
                    if self.peek() != Some(b')') {
                        loop {
                            args.push(self.pattern()?);
                            if self.peek() == Some(b',') {
                                self.at += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    if self.peek() != Some(b')') {
                        return Err(error("malformed pattern", self.span));
                    }
                    self.at += 1;
                }
                if path.len() == 1 {
                    let name = path.pop().expect("one segment");
                    return Ok(if name == "_" {
                        Pat::Wild
                    } else {
                        Pat::Bind(name)
                    });
                }
                let variant = path.pop().expect("at least two segments");
                Ok(Pat::Ctor {
                    enum_name: path.join("::"),
                    variant,
                    args,
                })
            }
        }
    }
}

fn literal_expression(text: &str, span: Span) -> Expression {
    match text {
        _ if text.starts_with('"') => {
            let bytes: Vec<u8> = text
                .trim_matches('"')
                .split('.')
                .filter(|part| !part.is_empty())
                .filter_map(|part| part.parse().ok())
                .collect();
            Expression::String(String::from_utf8_lossy(&bytes).into_owned(), span)
        }
        "true" => Expression::Boolean(true, span),
        "false" => Expression::Boolean(false, span),
        _ if text.starts_with('\'') => {
            let code: u32 = text.trim_matches('\'').parse().unwrap_or(0);
            Expression::Character(char::from_u32(code).unwrap_or('\0'), span)
        }
        _ => {
            let (negative, digits) = match text.strip_prefix('-') {
                Some(digits) => (true, digits),
                None => (false, text),
            };
            let value = Expression::Integer(digits.parse().unwrap_or(0), span);
            if negative {
                Expression::Negate(Box::new(value), span)
            } else {
                value
            }
        }
    }
}

fn compile_choose(
    value: Expression,
    arms: Vec<ChooseArm>,
    span: Span,
    fresh: &mut usize,
) -> Result<Expression, Diagnostic> {
    let root = match &value {
        Expression::Name(name, _) => name.clone(),
        _ => {
            *fresh += 1;
            format!("$m{fresh}")
        }
    };
    let root_expression = Expression::Name(root.clone(), span);
    let mut rows = Vec::new();
    for (index, arm) in arms.iter().enumerate() {
        let pattern = match (&arm.enum_name, &arm.variant) {
            (None, _) => Pat::Wild,
            (Some(name), Some(literal)) if name == "$lit" => Pat::Lit(literal.clone()),
            (Some(name), Some(text)) if name == "$pat" => parse_pattern(text, arm.span)?,
            (Some(enum_name), Some(variant)) => Pat::Ctor {
                enum_name: enum_name.clone(),
                variant: variant.clone(),
                args: arm
                    .bindings
                    .iter()
                    .map(|binding| parse_pattern(binding, arm.span))
                    .collect::<Result<_, _>>()?,
            },
            _ => return Err(error("malformed pattern", arm.span)),
        };
        rows.push(Row {
            pats: vec![pattern],
            binds: Vec::new(),
            body: arm.body.clone(),
            index,
            span: arm.span,
        });
    }
    let mut compiler = Compiler {
        used: vec![false; arms.len()],
        fresh: *fresh,
    };
    let compiled = compiler.compile(&[root_expression], rows, span)?;
    *fresh = compiler.fresh;
    for (index, used) in compiler.used.iter().enumerate() {
        if !used {
            return Err(error("this pattern can never match", arms[index].span)
                .with_help_text("an earlier pattern already covers every value it would match"));
        }
    }
    let Some(mut result) = compiled else {
        return Err(error("`choose` has no arms", span));
    };
    if matches!(&value, Expression::Name(..)) {
        return Ok(result);
    }
    // The root is only bound by the scrutinee itself; an arm that still names it (through a
    // shape probe or a bound pattern) needs the payload wrapper below.
    if let Expression::Choose {
        value: scrutinee,
        arms,
        ..
    } = &mut result
        && matches!(&**scrutinee, Expression::Name(name, _) if *name == root)
        && !arms.iter().any(|arm| uses_name(&arm.body, &root))
    {
        **scrutinee = value;
        return Ok(result);
    }
    // A literal chain tests the scrutinee several times: evaluate it once.
    let payload = format!("$w{}", compiler.fresh);
    compiler.fresh += 1;
    *fresh = compiler.fresh;
    let mut chain = result;
    let mut unreachable_chain = chain.clone();
    rename(&mut chain, &root, &payload);
    // The `None` arm never runs; it only has to type-check without the payload name.
    replace_name(&mut unreachable_chain, &root, &value);
    Ok(Expression::Choose {
        value: Box::new(Expression::EnumConstruct {
            enum_name: "Option".into(),
            variant: "Some".into(),
            arguments: vec![value.clone()],
            span,
        }),
        arms: vec![
            ChooseArm {
                enum_name: Some("Option".into()),
                variant: Some("Some".into()),
                bindings: vec![payload],
                body: chain.clone(),
                span,
            },
            ChooseArm {
                enum_name: Some("Option".into()),
                variant: Some("None".into()),
                bindings: Vec::new(),
                body: unreachable_chain,
                span,
            },
        ],
        span,
    })
}

/// Wraps the rows of a tuple or structure pattern in a probe that sema checks against the
/// scrutinee's type. Without it, a pattern whose fields are never read would not be checked at all.
/// Sema accepts the probe only over a value with the matching shape and then keeps just `body`.
fn shape_probe(
    value: &Expression,
    tuple_arity: Option<usize>,
    body: Expression,
    span: Span,
) -> Expression {
    let variant = match tuple_arity {
        Some(arity) => format!("tuple:{arity}"),
        None => "struct".to_owned(),
    };
    Expression::Choose {
        value: Box::new(value.clone()),
        arms: vec![ChooseArm {
            enum_name: Some("$shape".into()),
            variant: Some(variant),
            bindings: Vec::new(),
            body,
            span,
        }],
        span,
    }
}

/// Replaces the free uses of `from` in `expression` with a copy of `with`.
fn replace_name(expression: &mut Expression, from: &str, with: &Expression) {
    match expression {
        Expression::Name(name, _) if name == from => *expression = with.clone(),
        Expression::Choose { value, arms, .. } => {
            replace_name(value, from, with);
            for arm in arms {
                if !arm.bindings.iter().any(|binding| binding == from) {
                    replace_name(&mut arm.body, from, with);
                }
            }
        }
        other => {
            for child in children_mut(other) {
                replace_name(child, from, with);
            }
        }
    }
}

/// Whether `name` occurs free in `expression`.
fn uses_name(expression: &Expression, name: &str) -> bool {
    let mut copy = expression.clone();
    let mut found = false;
    fn walk(expression: &mut Expression, name: &str, found: &mut bool) {
        if matches!(expression, Expression::Name(candidate, _) if candidate == name) {
            *found = true;
        }
        for child in children_mut(expression) {
            walk(child, name, found);
        }
    }
    walk(&mut copy, name, &mut found);
    found
}

trait WithHelp {
    fn with_help_text(self, help: &str) -> Self;
}

impl WithHelp for Diagnostic {
    fn with_help_text(mut self, help: &str) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Renames the free uses of `from` in `expression` to `to`.
fn rename(expression: &mut Expression, from: &str, to: &str) {
    match expression {
        Expression::Name(name, _) if name == from => *name = to.to_owned(),
        Expression::Choose { value, arms, .. } => {
            rename(value, from, to);
            for arm in arms {
                if !arm.bindings.iter().any(|binding| binding == from) {
                    rename(&mut arm.body, from, to);
                }
            }
        }
        other => {
            for child in children_mut(other) {
                rename(child, from, to);
            }
        }
    }
}

impl Compiler {
    fn fresh_name(&mut self) -> String {
        self.fresh += 1;
        format!("$p{}", self.fresh)
    }

    fn field_of(value: &Expression, name: &str, span: Span) -> Expression {
        Expression::Field {
            value: Box::new(value.clone()),
            name: name.to_owned(),
            name_span: span,
            span,
        }
    }

    /// Compiles the rows against the scrutinee variables; `None` when no row can match.
    fn compile(
        &mut self,
        vars: &[Expression],
        rows: Vec<Row>,
        span: Span,
    ) -> Result<Option<Expression>, Diagnostic> {
        let Some(first) = rows.first() else {
            return Ok(None);
        };
        let Some(column) = first.pats.iter().position(|pattern| {
            matches!(
                pattern,
                Pat::Ctor { .. } | Pat::Lit(_) | Pat::Tuple(_) | Pat::Fields(_)
            )
        }) else {
            // The first row matches everything that reaches it.
            let mut row = rows.into_iter().next().expect("a first row exists");
            self.used[row.index] = true;
            for (position, pattern) in row.pats.iter().enumerate() {
                if let Pat::Bind(name) = pattern {
                    row.binds.push((name.clone(), vars[position].clone()));
                }
            }
            let mut body = row.body;
            for (name, var) in &row.binds {
                replace_name(&mut body, name, var);
            }
            return Ok(Some(body));
        };
        let remaining: Vec<Expression> = vars
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != column)
            .map(|(_, var)| var.clone())
            .collect();
        match &first.pats[column] {
            Pat::Ctor { .. } => self.switch_variants(vars, &remaining, column, rows, span),
            Pat::Tuple(_) | Pat::Fields(_) => {
                self.switch_product(vars, &remaining, column, rows, span)
            }
            _ => self.switch_literals(vars, &remaining, column, rows, span),
        }
    }

    fn without(pats: &[Pat], column: usize) -> Vec<Pat> {
        pats.iter()
            .enumerate()
            .filter(|(position, _)| *position != column)
            .map(|(_, pattern)| pattern.clone())
            .collect()
    }

    fn switch_variants(
        &mut self,
        vars: &[Expression],
        remaining: &[Expression],
        column: usize,
        rows: Vec<Row>,
        span: Span,
    ) -> Result<Option<Expression>, Diagnostic> {
        let mut constructors: Vec<(String, String, usize)> = Vec::new();
        for row in &rows {
            match &row.pats[column] {
                Pat::Ctor {
                    enum_name,
                    variant,
                    args,
                } => {
                    if let Some(known) = constructors
                        .iter()
                        .find(|(e, v, _)| e == enum_name && v == variant)
                    {
                        if known.2 != args.len() {
                            return Err(error(
                                format!("pattern `{enum_name}::{variant}` has inconsistent arity"),
                                row.span,
                            ));
                        }
                    } else {
                        constructors.push((enum_name.clone(), variant.clone(), args.len()));
                    }
                }
                Pat::Lit(_) | Pat::Tuple(_) | Pat::Fields(_) => {
                    return Err(error(
                        "a variant pattern cannot be mixed with other kinds of patterns",
                        row.span,
                    ));
                }
                Pat::Bind(name) => {
                    if uses_name(&row.body, name) {
                        return Err(error(
                            "a name cannot bind a value that other arms destructure",
                            row.span,
                        )
                        .with_help_text("use `_` or destructure the value in every arm"));
                    }
                }
                Pat::Wild => {}
            }
        }
        // A name the arm never uses matches like `_`.
        let rows: Vec<Row> = rows
            .into_iter()
            .map(|mut row| {
                if matches!(row.pats[column], Pat::Bind(_)) {
                    row.pats[column] = Pat::Wild;
                }
                row
            })
            .collect();
        let mut arms = Vec::new();
        for (enum_name, variant, arity) in &constructors {
            let fresh: Vec<String> = (0..*arity).map(|_| self.fresh_name()).collect();
            let fresh_vars: Vec<Expression> = fresh
                .iter()
                .map(|name| Expression::Name(name.clone(), span))
                .collect();
            let mut specialized = Vec::new();
            for row in &rows {
                let mut pats = match &row.pats[column] {
                    Pat::Ctor {
                        enum_name: e,
                        variant: v,
                        args,
                    } if e == enum_name && v == variant => args.clone(),
                    Pat::Wild => vec![Pat::Wild; *arity],
                    _ => continue,
                };
                pats.extend(Self::without(&row.pats, column));
                let mut row = row.clone();
                row.pats = pats;
                specialized.push(row);
            }
            let mut inner_vars = fresh_vars;
            inner_vars.extend_from_slice(remaining);
            let Some(body) = self.compile(&inner_vars, specialized, span)? else {
                continue;
            };
            arms.push(ChooseArm {
                enum_name: Some(enum_name.clone()),
                variant: Some(variant.clone()),
                bindings: fresh,
                body,
                span,
            });
        }
        let defaults: Vec<Row> = rows
            .iter()
            .filter(|row| matches!(row.pats[column], Pat::Wild))
            .map(|row| {
                let mut row = row.clone();
                row.pats = Self::without(&row.pats, column);
                row
            })
            .collect();
        if let Some(body) = self.compile(remaining, defaults, span)? {
            arms.push(ChooseArm {
                enum_name: None,
                variant: None,
                bindings: Vec::new(),
                body,
                span,
            });
        }
        Ok(Some(Expression::Choose {
            value: Box::new(vars[column].clone()),
            arms,
            span,
        }))
    }

    /// Tuples and structures have one constructor: expand the column into one column per field.
    fn switch_product(
        &mut self,
        vars: &[Expression],
        remaining: &[Expression],
        column: usize,
        rows: Vec<Row>,
        span: Span,
    ) -> Result<Option<Expression>, Diagnostic> {
        let mut names: Vec<String> = Vec::new();
        let mut tuple_arity: Option<usize> = None;
        for row in &rows {
            match &row.pats[column] {
                Pat::Tuple(elements) => {
                    if tuple_arity.is_some_and(|arity| arity != elements.len()) {
                        return Err(error("tuple patterns have different lengths", row.span));
                    }
                    tuple_arity = Some(elements.len());
                }
                Pat::Fields(fields) => {
                    for (name, _) in fields {
                        if !names.contains(name) {
                            names.push(name.clone());
                        }
                    }
                }
                Pat::Wild => {}
                _ => {
                    return Err(error(
                        "this pattern cannot be mixed with a tuple or structure pattern",
                        row.span,
                    ));
                }
            }
        }
        if let Some(arity) = tuple_arity {
            if !names.is_empty() {
                return Err(error(
                    "a tuple pattern cannot be mixed with a structure pattern",
                    span,
                ));
            }
            names = (0..arity).map(|index| format!("_{index}")).collect();
        }
        let mut inner_vars: Vec<Expression> = names
            .iter()
            .map(|name| Self::field_of(&vars[column], name, span))
            .collect();
        inner_vars.extend_from_slice(remaining);
        let specialized: Vec<Row> = rows
            .iter()
            .map(|row| {
                let mut pats: Vec<Pat> = match &row.pats[column] {
                    Pat::Tuple(elements) => elements.clone(),
                    Pat::Fields(fields) => names
                        .iter()
                        .map(|name| {
                            fields
                                .iter()
                                .find(|(candidate, _)| candidate == name)
                                .map_or(Pat::Wild, |(_, pattern)| pattern.clone())
                        })
                        .collect(),
                    _ => vec![Pat::Wild; names.len()],
                };
                pats.extend(Self::without(&row.pats, column));
                let mut row = row.clone();
                row.pats = pats;
                row
            })
            .collect();
        let body = self.compile(&inner_vars, specialized, span)?;
        Ok(body.map(|body| shape_probe(&vars[column], tuple_arity, body, span)))
    }

    fn switch_literals(
        &mut self,
        vars: &[Expression],
        remaining: &[Expression],
        column: usize,
        rows: Vec<Row>,
        span: Span,
    ) -> Result<Option<Expression>, Diagnostic> {
        let mut literals: Vec<String> = Vec::new();
        for row in &rows {
            match &row.pats[column] {
                Pat::Lit(text) => {
                    if !literals.contains(text) {
                        literals.push(text.clone());
                    }
                }
                Pat::Ctor { .. } | Pat::Tuple(_) | Pat::Fields(_) => {
                    return Err(error(
                        "a literal pattern cannot be mixed with other kinds of patterns",
                        row.span,
                    ));
                }
                Pat::Wild | Pat::Bind(_) => {}
            }
        }
        let bind = |row: &Row, pats: Vec<Pat>| {
            let mut row = row.clone();
            if let Pat::Bind(name) = &row.pats[column] {
                row.binds.push((name.clone(), vars[column].clone()));
            }
            row.pats = pats;
            row
        };
        let defaults: Vec<Row> = rows
            .iter()
            .filter(|row| matches!(row.pats[column], Pat::Wild | Pat::Bind(_)))
            .map(|row| bind(row, Self::without(&row.pats, column)))
            .collect();
        let boolean_complete = literals.len() == 2
            && literals
                .iter()
                .all(|text| text == "true" || text == "false")
            && defaults.is_empty();
        let mut tests = literals.clone();
        let mut otherwise = None;
        if boolean_complete {
            // With both booleans covered, the last one is the `else` branch.
            let last = tests.pop().expect("two literals");
            let matching: Vec<Row> = rows
                .iter()
                .filter(|row| row.pats[column] == Pat::Lit(last.clone()))
                .map(|row| bind(row, Self::without(&row.pats, column)))
                .collect();
            otherwise = self.compile(remaining, matching, span)?;
        } else if let Some(body) = self.compile(remaining, defaults, span)? {
            otherwise = Some(body);
        }
        let Some(mut chain) = otherwise else {
            let blame = rows.first().map_or(span, |row| row.span);
            return Err(
                error("literal patterns must be followed by a `_` arm", blame)
                    .with_help_text("integers and characters cannot be matched exhaustively"),
            );
        };
        for text in tests.iter().rev() {
            let matching: Vec<Row> = rows
                .iter()
                .filter(|row| match &row.pats[column] {
                    Pat::Lit(candidate) => candidate == text,
                    _ => true,
                })
                .map(|row| bind(row, Self::without(&row.pats, column)))
                .collect();
            let then_value = self
                .compile(remaining, matching, span)?
                .expect("a literal row exists");
            chain = Expression::If {
                condition: Box::new(Expression::Binary {
                    op: BinaryOp::Eq,
                    left: Box::new(vars[column].clone()),
                    right: Box::new(literal_expression(text, span)),
                    span,
                }),
                then_value: Box::new(then_value),
                else_value: Box::new(chain),
                span,
            };
        }
        Ok(Some(chain))
    }
}
