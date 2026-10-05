use crate::{
    ast::{
        BinaryOp, Expression, Function, Parameter, PrintPart, Program, Statement, StructDef,
        StructField, TypeName,
    },
    lexer::{Token, TokenKind, lex, lex_recovering},
    source::{Diagnostic, Span},
};

// Keep recursive expression parsing comfortably below the default Windows
// process-thread stack limit while still allowing ordinary deep expressions.
const MAX_EXPRESSION_DEPTH: usize = 64;
const MAX_BLOCK_DEPTH: usize = 128;
const MAX_IF_DEPTH: usize = 128;

pub fn parse(text: &str) -> Result<Program, Diagnostic> {
    Parser::new(text)?.program()
}

/// Parses a source file and gathers recoverable lexical errors. If lexing
/// succeeds, it recovers between structure fields, function parameters,
/// arguments, structure literal fields, statements, and top-level declarations.
/// It also continues into control-flow bodies after malformed conditions or
/// range headers to report independent syntax errors together.
pub fn parse_recovering(text: &str) -> Result<Program, Vec<Diagnostic>> {
    let parser = Parser::new_recovering(text)?;
    parser.program_recovering()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    at: usize,
    expression_depth: usize,
    block_depth: usize,
    if_depth: usize,
    speculative: bool,
    recovering: bool,
    recovery_diagnostics: Vec<Diagnostic>,
    control_history: Vec<(usize, ControlMarker)>,
}

#[derive(Clone, Copy)]
enum ControlMarker {
    If,
    Else,
    LBrace,
}

impl Parser<'_> {
    fn new(source: &str) -> Result<Parser<'_>, Diagnostic> {
        Ok(Parser::with_tokens(source, lex(source)?))
    }

    fn new_recovering(source: &str) -> Result<Parser<'_>, Vec<Diagnostic>> {
        let mut parser = Parser::with_tokens(source, lex_recovering(source)?);
        parser.recovering = true;
        Ok(parser)
    }

    fn with_tokens(source: &str, tokens: Vec<Token>) -> Parser<'_> {
        Parser {
            source,
            tokens,
            at: 0,
            expression_depth: 0,
            block_depth: 0,
            if_depth: 0,
            speculative: false,
            recovering: false,
            recovery_diagnostics: Vec::new(),
            control_history: Vec::new(),
        }
    }

    fn program(mut self) -> Result<Program, Diagnostic> {
        let mut structs = Vec::new();
        let mut functions = Vec::new();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            if matches!(self.peek().kind, TokenKind::Struct) {
                structs.push(self.struct_def()?);
            } else {
                functions.push(self.function()?);
            }
        }
        Ok(Program { structs, functions })
    }

    fn program_recovering(mut self) -> Result<Program, Vec<Diagnostic>> {
        let mut structs = Vec::new();
        let mut functions = Vec::new();
        let mut diagnostics = Vec::new();

        while !matches!(self.peek().kind, TokenKind::Eof) {
            let declaration_start = self.at;
            let result = if matches!(self.peek().kind, TokenKind::Struct) {
                self.struct_def().map(|definition| structs.push(definition))
            } else {
                self.function().map(|function| functions.push(function))
            };
            diagnostics.append(&mut self.recovery_diagnostics);

            match result {
                Ok(()) => {}
                Err(diagnostic) => {
                    diagnostics.push(diagnostic);
                    if self.at == declaration_start {
                        self.next();
                    }
                    self.synchronize_top_level();
                }
            }
        }

        if diagnostics.is_empty() {
            Ok(Program { structs, functions })
        } else {
            Err(diagnostics)
        }
    }

    fn synchronize_top_level(&mut self) {
        while !matches!(
            self.peek().kind,
            TokenKind::Eof | TokenKind::Fn | TokenKind::Struct
        ) {
            self.next();
        }
    }

    fn synchronize_statement(&mut self, start: usize, allow_call: bool) {
        let mut brace_depth = 0;
        let mut paren_depth = 0;
        while !matches!(self.peek().kind, TokenKind::Eof) {
            if self.at > start
                && brace_depth == 0
                && paren_depth == 0
                && (matches!(self.peek().kind, TokenKind::RBrace)
                    || self.starts_statement(allow_call))
            {
                return;
            }
            match self.peek().kind {
                TokenKind::LBrace => brace_depth += 1,
                TokenKind::RBrace if brace_depth > 0 => brace_depth -= 1,
                TokenKind::RBrace => return,
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen if paren_depth > 0 => paren_depth -= 1,
                _ => {}
            }
            self.next();
        }
    }

    fn synchronize_struct_field(&mut self, start: usize) {
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            if self.at > start
                && matches!(self.peek().kind, TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.at + 1).map(|token| &token.kind),
                    Some(TokenKind::Colon)
                )
            {
                return;
            }
            if matches!(self.peek().kind, TokenKind::Comma) {
                self.next();
                return;
            }
            self.next();
        }
    }

    fn synchronize_parameter(&mut self, start: usize) {
        while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
            if self.at > start
                && matches!(self.peek().kind, TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.at + 1).map(|token| &token.kind),
                    Some(TokenKind::Colon)
                )
            {
                return;
            }
            if matches!(self.peek().kind, TokenKind::Comma) {
                self.next();
                return;
            }
            self.next();
        }
    }

    fn diagnostic_token_was_consumed(&self, start: usize, span: Span) -> bool {
        self.at > start && self.tokens[self.at - 1].span == span
    }

    fn synchronize_expression_list(&mut self, closing: TokenKind) {
        let mut paren_depth = 0;
        let mut brace_depth = 0;
        while !matches!(self.peek().kind, TokenKind::Eof) {
            if paren_depth == 0 && brace_depth == 0 {
                if self.peek().kind == closing || matches!(self.peek().kind, TokenKind::RBrace) {
                    return;
                }
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                    return;
                }
            }
            match self.peek().kind {
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen if paren_depth > 0 => paren_depth -= 1,
                TokenKind::LBrace => brace_depth += 1,
                TokenKind::RBrace if brace_depth > 0 => brace_depth -= 1,
                _ => {}
            }
            self.next();
        }
    }

    fn struct_def(&mut self) -> Result<StructDef, Diagnostic> {
        let start = self.next().span.start;
        let (name, _) = self.ident("expected structure name")?;
        self.expect(
            |kind| matches!(kind, TokenKind::LBrace),
            "expected `{` after structure name",
        )?;
        let mut fields = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            let field_start = self.at;
            let field = (|| {
                let (field_name, field_span) = self.ident("expected field name")?;
                self.expect(
                    |kind| matches!(kind, TokenKind::Colon),
                    "expected `:` after field name",
                )?;
                let ty = self.type_name()?;
                Ok(StructField {
                    name: field_name,
                    ty,
                    span: field_span,
                })
            })();
            match field {
                Ok(field) => fields.push(field),
                Err(diagnostic) if self.recovering => {
                    self.recovery_diagnostics.push(diagnostic);
                    self.synchronize_struct_field(field_start);
                }
                Err(diagnostic) => return Err(diagnostic),
            }
            if matches!(self.peek().kind, TokenKind::Comma) {
                self.next();
            } else if !matches!(self.peek().kind, TokenKind::Ident(_)) {
                break;
            }
        }
        let end = self
            .expect(
                |kind| matches!(kind, TokenKind::RBrace),
                "expected `}` after structure fields",
            )?
            .span
            .end;
        Ok(StructDef {
            name,
            fields,
            span: Span { start, end },
        })
    }

    fn function(&mut self) -> Result<Function, Diagnostic> {
        let start = self
            .expect(|k| matches!(k, TokenKind::Fn), "expected `fn`")?
            .span
            .start;
        let name = match self.next().kind {
            TokenKind::Ident(name) => name,
            _ => return Err(self.error("expected function name")),
        };
        self.expect(
            |k| matches!(k, TokenKind::LParen),
            "expected `(` after function name",
        )?;
        let mut parameters = Vec::new();
        if !matches!(self.peek().kind, TokenKind::RParen) {
            while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
                let parameter_start = self.at;
                let parameter = (|| {
                    let (param_name, param_span) = self.ident("expected parameter name")?;
                    self.expect(
                        |kind| matches!(kind, TokenKind::Colon),
                        "expected `:` after parameter name",
                    )?;
                    let ty = self.type_name()?;
                    Ok(Parameter {
                        name: param_name,
                        ty,
                        span: param_span,
                    })
                })();
                match parameter {
                    Ok(parameter) => parameters.push(parameter),
                    Err(diagnostic) if self.recovering => {
                        self.recovery_diagnostics.push(diagnostic);
                        self.synchronize_parameter(parameter_start);
                        continue;
                    }
                    Err(diagnostic) => return Err(diagnostic),
                }
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                    if matches!(self.peek().kind, TokenKind::RParen) {
                        let diagnostic = self.error("expected parameter name");
                        if self.recovering {
                            self.recovery_diagnostics.push(diagnostic);
                            break;
                        }
                        return Err(diagnostic);
                    }
                } else if matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
                    break;
                } else if self.recovering {
                    self.recovery_diagnostics
                        .push(self.error("expected `,` between function parameters"));
                    self.synchronize_parameter(parameter_start);
                } else {
                    break;
                }
            }
        }
        self.expect(
            |k| matches!(k, TokenKind::RParen),
            "expected `)` after parameters",
        )?;
        let return_type = if matches!(self.peek().kind, TokenKind::Arrow) {
            self.next();
            Some(self.type_name()?)
        } else {
            None
        };
        self.expect(|k| matches!(k, TokenKind::LBrace), "expected function body")?;
        let mut body = Vec::new();
        let mut tail_return = None;
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            let statement_start = self.at;
            let starts_statement = self.starts_statement(return_type.is_none())
                || self.call_is_statement_before_result(return_type.is_some());
            let is_tail_if_expression = return_type.is_some()
                && matches!(self.peek().kind, TokenKind::If)
                && self.if_expression_reaches_function_end();
            if starts_statement && !is_tail_if_expression {
                match self.statement() {
                    Ok(statement) => body.push(statement),
                    Err(diagnostic) if self.recovering => {
                        self.recovery_diagnostics.push(diagnostic);
                        self.synchronize_statement(statement_start, return_type.is_none());
                    }
                    Err(diagnostic) => return Err(diagnostic),
                }
            } else {
                match self.expression(0) {
                    Ok(expression) => {
                        tail_return = Some(expression);
                        break;
                    }
                    Err(diagnostic) if self.recovering => {
                        self.recovery_diagnostics.push(diagnostic);
                        self.synchronize_statement(statement_start, return_type.is_none());
                    }
                    Err(diagnostic) => return Err(diagnostic),
                }
            }
        }
        let return_value = tail_return;
        let end = self
            .expect(
                |k| matches!(k, TokenKind::RBrace),
                "expected `}` to close function",
            )?
            .span
            .end;
        Ok(Function {
            name,
            parameters,
            return_type,
            body,
            return_value,
            span: Span { start, end },
        })
    }

    fn type_name(&mut self) -> Result<TypeName, Diagnostic> {
        let (name, span) = self.ident("expected type name")?;
        match name.as_str() {
            "i8" => Ok(TypeName::I8),
            "i16" => Ok(TypeName::I16),
            "i32" => Ok(TypeName::I32),
            "i64" => Ok(TypeName::I64),
            "u8" => Ok(TypeName::U8),
            "u16" => Ok(TypeName::U16),
            "u32" => Ok(TypeName::U32),
            "u64" => Ok(TypeName::U64),
            "f32" => Ok(TypeName::F32),
            "f64" => Ok(TypeName::F64),
            "str" => Ok(TypeName::Str),
            "bool" => Ok(TypeName::Bool),
            _ => Ok(TypeName::Named(name, span)),
        }
    }

    fn starts_statement(&self, allow_call: bool) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Let
                | TokenKind::Print
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Return
        ) || self.field_assignment_start()
            || (matches!(self.peek().kind, TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(
                        TokenKind::Equal
                            | TokenKind::PlusEqual
                            | TokenKind::MinusEqual
                            | TokenKind::StarEqual
                            | TokenKind::SlashEqual
                            | TokenKind::PercentEqual
                            | TokenKind::BitAndEqual
                            | TokenKind::BitOrEqual
                            | TokenKind::CaretEqual
                            | TokenKind::ShiftLeftEqual
                            | TokenKind::ShiftRightEqual
                    )
                ))
            || (allow_call
                && matches!(self.peek().kind, TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                ))
    }

    fn starts_expression(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::If
                | TokenKind::Ident(_)
                | TokenKind::Minus
                | TokenKind::Bang
                | TokenKind::LParen
        )
    }

    fn field_assignment_start(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_)) {
            return false;
        }
        let mut index = self.at + 1;
        let mut has_field = false;
        while matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Dot)
        ) {
            has_field = true;
            if !matches!(
                self.tokens.get(index + 1).map(|token| &token.kind),
                Some(TokenKind::Ident(_))
            ) {
                return false;
            }
            index += 2;
        }
        has_field
            && matches!(
                self.tokens.get(index).map(|token| &token.kind),
                Some(
                    TokenKind::Equal
                        | TokenKind::PlusEqual
                        | TokenKind::MinusEqual
                        | TokenKind::StarEqual
                        | TokenKind::SlashEqual
                        | TokenKind::PercentEqual
                        | TokenKind::BitAndEqual
                        | TokenKind::BitOrEqual
                        | TokenKind::CaretEqual
                        | TokenKind::ShiftLeftEqual
                        | TokenKind::ShiftRightEqual
                )
            )
    }

    fn call_is_statement_before_result(&self, has_return_type: bool) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_))
            || !matches!(
                self.tokens.get(self.at + 1).map(|token| &token.kind),
                Some(TokenKind::LParen)
            )
        {
            return false;
        }

        let mut depth = 0usize;
        for (index, token) in self.tokens.iter().enumerate().skip(self.at + 1) {
            match token.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return match self.tokens.get(index + 1).map(|token| &token.kind) {
                            Some(TokenKind::RBrace) => !has_return_type,
                            Some(
                                TokenKind::Plus
                                | TokenKind::Minus
                                | TokenKind::Star
                                | TokenKind::Slash
                                | TokenKind::Percent
                                | TokenKind::EqualEqual
                                | TokenKind::BangEqual
                                | TokenKind::Less
                                | TokenKind::LessEqual
                                | TokenKind::Greater
                                | TokenKind::GreaterEqual
                                | TokenKind::BitAnd
                                | TokenKind::BitOr
                                | TokenKind::Caret
                                | TokenKind::ShiftLeft
                                | TokenKind::ShiftRight
                                | TokenKind::As
                                | TokenKind::AndAnd
                                | TokenKind::OrOr,
                            ) => false,
                            _ => true,
                        };
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }

    fn statement(&mut self) -> Result<Statement, Diagnostic> {
        match self.peek().kind {
            TokenKind::Let => self.let_statement(),
            TokenKind::Print => self.print_statement(),
            TokenKind::If => self.if_statement(),
            TokenKind::While => self.while_statement(),
            TokenKind::For => self.for_statement(),
            TokenKind::Break => Ok(Statement::Break(self.next().span)),
            TokenKind::Continue => Ok(Statement::Continue(self.next().span)),
            TokenKind::Return => self.return_statement(),
            TokenKind::Ident(_) if self.field_assignment_start() => self.field_assignment(),
            TokenKind::Ident(_)
                if matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(
                        TokenKind::Equal
                            | TokenKind::PlusEqual
                            | TokenKind::MinusEqual
                            | TokenKind::StarEqual
                            | TokenKind::SlashEqual
                            | TokenKind::PercentEqual
                            | TokenKind::BitAndEqual
                            | TokenKind::BitOrEqual
                            | TokenKind::CaretEqual
                            | TokenKind::ShiftLeftEqual
                            | TokenKind::ShiftRightEqual
                    )
                ) =>
            {
                self.assignment()
            }
            TokenKind::Ident(_)
                if matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                ) =>
            {
                self.call_statement()
            }
            _ => Err(self.error(
                "expected `let`, assignment, function call, `break`, `continue`, `return`, or `print` statement",
            )),
        }
    }

    fn return_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let value = if matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            None
        } else {
            Some(self.expression(0)?)
        };
        let end = value
            .as_ref()
            .map_or(self.tokens[self.at - 1].span.end, |value| value.span().end);
        let span = Span { start, end };
        Ok(Statement::Return { value, span })
    }

    fn call_statement(&mut self) -> Result<Statement, Diagnostic> {
        match self.expression(0)? {
            Expression::Call {
                name,
                arguments,
                span,
            } => Ok(Statement::Call {
                name,
                arguments,
                span,
            }),
            _ => Err(self.error("only a function call can be used as an expression statement")),
        }
    }

    fn if_statement(&mut self) -> Result<Statement, Diagnostic> {
        if self.if_depth >= MAX_IF_DEPTH {
            return Err(self.nesting_error("conditional"));
        }
        self.if_depth += 1;
        let result = self.if_statement_inner();
        self.if_depth -= 1;
        result
    }

    fn if_statement_inner(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let (condition, opening_brace_consumed) = self.condition_before_block()?;
        let then_body = if opening_brace_consumed {
            self.block_statements_after_open()?
        } else {
            self.block_statements()?
        };
        let else_body = if matches!(self.peek().kind, TokenKind::Else) {
            self.next();
            if matches!(self.peek().kind, TokenKind::If) {
                vec![self.if_statement()?]
            } else {
                self.block_statements()?
            }
        } else {
            Vec::new()
        };
        let end = self.tokens[self.at - 1].span.end;
        Ok(Statement::If {
            condition,
            then_body,
            else_body,
            span: Span { start, end },
        })
    }

    fn condition_before_block(&mut self) -> Result<(Expression, bool), Diagnostic> {
        let expression_start = self.at;
        match self.expression(0) {
            Ok(condition) => Ok((condition, false)),
            Err(diagnostic) if self.recovering => {
                let brace_consumed = self
                    .diagnostic_token_was_consumed(expression_start, diagnostic.span)
                    && self.source.get(diagnostic.span.start..diagnostic.span.end) == Some("{");
                let span = diagnostic.span;
                self.recovery_diagnostics.push(diagnostic);
                if !brace_consumed {
                    self.synchronize_control_body(expression_start);
                }
                Ok((Expression::Boolean(false, span), brace_consumed))
            }
            Err(diagnostic) => Err(diagnostic),
        }
    }

    fn block_statements(&mut self) -> Result<Vec<Statement>, Diagnostic> {
        self.block_statements_with_opening(false)
    }

    fn synchronize_control_body(&mut self, header_start: usize) {
        let mut pending_if_body = false;
        let history_start = self
            .control_history
            .partition_point(|(index, _)| *index < header_start);
        for (_, marker) in self.control_history[history_start..]
            .iter()
            .take_while(|(index, _)| *index < self.at)
        {
            match marker {
                ControlMarker::If => pending_if_body = true,
                ControlMarker::LBrace | ControlMarker::Else => pending_if_body = false,
            }
        }

        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            if matches!(self.peek().kind, TokenKind::Else) {
                self.next();
                if matches!(self.peek().kind, TokenKind::If) {
                    self.next();
                    pending_if_body = true;
                    continue;
                }
                if matches!(self.peek().kind, TokenKind::LBrace) {
                    self.skip_balanced_block();
                }
                pending_if_body = false;
                continue;
            }
            if matches!(self.peek().kind, TokenKind::LBrace) {
                if pending_if_body {
                    self.skip_balanced_block();
                    pending_if_body = false;
                    continue;
                }
                break;
            }
            if matches!(self.peek().kind, TokenKind::If) {
                self.next();
                pending_if_body = true;
                continue;
            }
            self.next();
        }
    }

    fn skip_balanced_block(&mut self) {
        let mut depth = 0usize;
        while !matches!(self.peek().kind, TokenKind::Eof) {
            match self.next().kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    fn block_statements_after_open(&mut self) -> Result<Vec<Statement>, Diagnostic> {
        self.block_statements_with_opening(true)
    }

    fn block_statements_with_opening(
        &mut self,
        opening_consumed: bool,
    ) -> Result<Vec<Statement>, Diagnostic> {
        if self.block_depth >= MAX_BLOCK_DEPTH {
            return Err(self.nesting_error("block"));
        }
        self.block_depth += 1;
        let result = if opening_consumed {
            self.block_statements_contents()
        } else {
            self.block_statements_inner()
        };
        self.block_depth -= 1;
        result
    }

    fn block_statements_inner(&mut self) -> Result<Vec<Statement>, Diagnostic> {
        self.expect(
            |k| matches!(k, TokenKind::LBrace),
            "expected `{` to start block",
        )?;
        self.block_statements_contents()
    }

    fn block_statements_contents(&mut self) -> Result<Vec<Statement>, Diagnostic> {
        let mut statements = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            let statement_start = self.at;
            match self.statement() {
                Ok(statement) => statements.push(statement),
                Err(diagnostic) if self.recovering => {
                    self.recovery_diagnostics.push(diagnostic);
                    self.synchronize_statement(statement_start, true);
                }
                Err(diagnostic) => return Err(diagnostic),
            }
        }
        self.expect(
            |k| matches!(k, TokenKind::RBrace),
            "expected `}` to close block",
        )?;
        Ok(statements)
    }

    fn while_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let (condition, opening_brace_consumed) = self.condition_before_block()?;
        let body = if opening_brace_consumed {
            self.block_statements_after_open()?
        } else {
            self.block_statements()?
        };
        let end = self.tokens[self.at - 1].span.end;
        Ok(Statement::While {
            condition,
            body,
            span: Span { start, end },
        })
    }

    fn for_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let header_start = self.at;
        let header: Result<(String, Span, Expression, Expression), Diagnostic> = (|| {
            let (name, name_span) = self.ident("expected loop variable after `for`")?;
            self.expect(
                |kind| matches!(kind, TokenKind::In),
                "expected `in` after the `for` loop variable",
            )?;
            let range_start = self.expression(0)?;
            self.expect(
                |kind| matches!(kind, TokenKind::DotDot),
                "expected `..` between the range start and end",
            )?;
            let range_end = self.expression(0)?;
            Ok((name, name_span, range_start, range_end))
        })();
        let (name, name_span, range_start, range_end, body) = match header {
            Ok((name, name_span, range_start, range_end)) => {
                let body = self.block_statements()?;
                (name, name_span, range_start, range_end, body)
            }
            Err(diagnostic) if self.recovering => {
                let brace_consumed = self
                    .diagnostic_token_was_consumed(header_start, diagnostic.span)
                    && self.source.get(diagnostic.span.start..diagnostic.span.end) == Some("{");
                let error_span = diagnostic.span;
                self.recovery_diagnostics.push(diagnostic);
                if !brace_consumed {
                    self.synchronize_control_body(header_start);
                }
                let body = if brace_consumed {
                    self.block_statements_after_open()?
                } else if matches!(self.peek().kind, TokenKind::LBrace) {
                    self.block_statements()?
                } else {
                    Vec::new()
                };
                let placeholder = Expression::Integer(0, error_span);
                (
                    "_error".into(),
                    error_span,
                    placeholder,
                    Expression::Integer(0, error_span),
                    body,
                )
            }
            Err(diagnostic) => return Err(diagnostic),
        };
        let end = self.tokens[self.at - 1].span.end;
        Ok(Statement::For {
            name,
            name_span,
            start: range_start,
            end: range_end,
            body,
            span: Span { start, end },
        })
    }

    fn let_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let mutable = if matches!(self.peek().kind, TokenKind::Mut) {
            self.next();
            true
        } else {
            false
        };
        let (name, _) = self.ident("expected variable name after `let`")?;
        let annotation = if matches!(self.peek().kind, TokenKind::Colon) {
            self.next();
            Some(self.type_name()?)
        } else {
            None
        };
        self.expect(
            |k| matches!(k, TokenKind::Equal),
            "expected `=` in variable declaration",
        )?;
        let value = self.expression(0)?;
        let end = value.span().end;
        Ok(Statement::Let {
            name,
            mutable,
            annotation,
            value,
            span: Span { start, end },
        })
    }

    fn assignment(&mut self) -> Result<Statement, Diagnostic> {
        let (name, start) = self.ident("expected variable name")?;
        let assignment = self.next();
        let value = self.expression(0)?;
        let end = value.span().end;
        let span = Span {
            start: start.start,
            end,
        };
        match assignment.kind {
            TokenKind::Equal => Ok(Statement::Assign { name, value, span }),
            TokenKind::PlusEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::Add,
                value,
                span,
            }),
            TokenKind::MinusEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::Sub,
                value,
                span,
            }),
            TokenKind::StarEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::Mul,
                value,
                span,
            }),
            TokenKind::SlashEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::Div,
                value,
                span,
            }),
            TokenKind::PercentEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::Rem,
                value,
                span,
            }),
            TokenKind::BitAndEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::BitAnd,
                value,
                span,
            }),
            TokenKind::BitOrEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::BitOr,
                value,
                span,
            }),
            TokenKind::CaretEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::BitXor,
                value,
                span,
            }),
            TokenKind::ShiftLeftEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::ShiftLeft,
                value,
                span,
            }),
            TokenKind::ShiftRightEqual => Ok(Statement::CompoundAssign {
                name,
                op: BinaryOp::ShiftRight,
                value,
                span,
            }),
            _ => Err(Diagnostic {
                code: "R0010",
                message: "expected an assignment operator".into(),
                span: assignment.span,
                help: None,
            }),
        }
    }

    fn field_assignment(&mut self) -> Result<Statement, Diagnostic> {
        let (object, object_span) = self.ident("expected variable name")?;
        let mut fields = Vec::new();
        while matches!(self.peek().kind, TokenKind::Dot) {
            if fields.len() >= MAX_EXPRESSION_DEPTH {
                return Err(self.nesting_error("field access"));
            }
            self.next();
            let (field, field_span) = self.ident("expected field name after `.`")?;
            fields.push((field, field_span));
        }
        let assignment = self.next();
        let op = match assignment.kind {
            TokenKind::Equal => None,
            TokenKind::PlusEqual => Some(BinaryOp::Add),
            TokenKind::MinusEqual => Some(BinaryOp::Sub),
            TokenKind::StarEqual => Some(BinaryOp::Mul),
            TokenKind::SlashEqual => Some(BinaryOp::Div),
            TokenKind::PercentEqual => Some(BinaryOp::Rem),
            TokenKind::BitAndEqual => Some(BinaryOp::BitAnd),
            TokenKind::BitOrEqual => Some(BinaryOp::BitOr),
            TokenKind::CaretEqual => Some(BinaryOp::BitXor),
            TokenKind::ShiftLeftEqual => Some(BinaryOp::ShiftLeft),
            TokenKind::ShiftRightEqual => Some(BinaryOp::ShiftRight),
            _ => {
                return Err(Diagnostic {
                    code: "R0010",
                    message: "expected an assignment operator after field name".into(),
                    span: assignment.span,
                    help: None,
                });
            }
        };
        let value = self.expression(0)?;
        let span = Span {
            start: object_span.start,
            end: value.span().end,
        };
        Ok(Statement::FieldAssign {
            object,
            fields,
            op,
            value,
            span,
        })
    }

    fn print_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        self.expect(
            |k| matches!(k, TokenKind::LParen),
            "expected `(` after `print`",
        )?;
        if let TokenKind::String(value) = &self.peek().kind
            && (value.contains('{') || value.contains('}'))
        {
            let token = self.next();
            let TokenKind::String(value) = token.kind else {
                return Err(Diagnostic {
                    code: "R0010",
                    message: "expected string literal after `print(`".into(),
                    span: token.span,
                    help: None,
                });
            };
            let Some(parts) = self.print_template(&value, token.span)? else {
                return Err(Diagnostic {
                    code: "R0010",
                    message: "expected print interpolation".into(),
                    span: token.span,
                    help: None,
                });
            };
            let end = self
                .expect(
                    |k| matches!(k, TokenKind::RParen),
                    "expected `)` after print value",
                )?
                .span
                .end;
            return Ok(Statement::PrintTemplate(parts, Span { start, end }));
        }
        let value = self.expression(0)?;
        self.expect(
            |k| matches!(k, TokenKind::RParen),
            "expected `)` after print value",
        )?;
        let end = self.tokens[self.at - 1].span.end;
        Ok(Statement::Print(value, Span { start, end }))
    }

    fn print_template(
        &self,
        value: &str,
        string_span: Span,
    ) -> Result<Option<Vec<PrintPart>>, Diagnostic> {
        let raw_start = string_span.start + 1;
        let raw_end = string_span.end.saturating_sub(1);
        let raw = self.source.get(raw_start..raw_end).unwrap_or("");
        let mut offsets = Vec::with_capacity(value.len() + 1);
        let mut raw_at = 0;
        while raw_at < raw.len() {
            let byte = raw.as_bytes()[raw_at];
            if byte == b'\\' {
                raw_at += 1;
                offsets.push(raw_start + raw_at);
                raw_at += usize::from(raw_at < raw.len());
            } else {
                let ch = raw[raw_at..].chars().next().expect("valid source boundary");
                for sub_byte in 0..ch.len_utf8() {
                    offsets.push(raw_start + raw_at + sub_byte);
                }
                raw_at += ch.len_utf8();
            }
        }
        offsets.push(raw_end);

        let bytes = value.as_bytes();
        let mut at = 0;
        let mut text = String::new();
        let mut parts = Vec::new();
        let mut used_template_syntax = false;
        while at < bytes.len() {
            match bytes[at] {
                b'{' if bytes.get(at + 1) == Some(&b'{') => {
                    text.push('{');
                    at += 2;
                    used_template_syntax = true;
                }
                b'}' if bytes.get(at + 1) == Some(&b'}') => {
                    text.push('}');
                    at += 2;
                    used_template_syntax = true;
                }
                b'{' => {
                    let open = at;
                    let Some(relative_end) = value[at + 1..].find('}') else {
                        return Err(Diagnostic {
                            code: "R0014",
                            message: "unterminated print interpolation".into(),
                            span: mapped_span(&offsets, raw_end, open, bytes.len()),
                            help: Some("close the interpolation with `}`".into()),
                        });
                    };
                    let name_start = open + 1;
                    let name_end = name_start + relative_end;
                    let name = &value[name_start..name_end];
                    if !is_identifier(name) {
                        return Err(Diagnostic {
                            code: "R0014",
                            message: "print interpolation must contain a variable name".into(),
                            span: mapped_span(&offsets, raw_end, open, name_end + 1),
                            help: Some("use a simple variable name, such as `{name}`".into()),
                        });
                    }
                    if !text.is_empty() {
                        parts.push(PrintPart::Text(std::mem::take(&mut text)));
                    }
                    parts.push(PrintPart::Value(Expression::Name(
                        name.to_owned(),
                        mapped_span(&offsets, raw_end, name_start, name_end),
                    )));
                    at = name_end + 1;
                    used_template_syntax = true;
                }
                b'}' => {
                    return Err(Diagnostic {
                        code: "R0014",
                        message: "unmatched `}` in print interpolation".into(),
                        span: mapped_span(&offsets, raw_end, at, at + 1),
                        help: Some("write `}}` to print a literal `}`".into()),
                    });
                }
                _ => {
                    let ch = value[at..].chars().next().expect("valid string boundary");
                    text.push(ch);
                    at += ch.len_utf8();
                }
            }
        }
        if !used_template_syntax {
            return Ok(None);
        }
        if !text.is_empty() {
            parts.push(PrintPart::Text(text));
        }
        Ok(Some(parts))
    }

    fn expression(&mut self, min_precedence: u8) -> Result<Expression, Diagnostic> {
        if self.expression_depth >= MAX_EXPRESSION_DEPTH {
            return Err(self.nesting_error("expression"));
        }
        self.expression_depth += 1;
        let result = self.expression_inner(min_precedence);
        self.expression_depth -= 1;
        result
    }

    fn expression_inner(&mut self, min_precedence: u8) -> Result<Expression, Diagnostic> {
        let mut left = match self.next() {
            Token {
                kind: TokenKind::Integer(value),
                span,
            } => Expression::Integer(value, span),
            Token {
                kind: TokenKind::Float(value),
                span,
            } => Expression::Float(value, span),
            Token {
                kind: TokenKind::String(value),
                span,
            } => Expression::String(value, span),
            Token {
                kind: TokenKind::True,
                span,
            } => Expression::Boolean(true, span),
            Token {
                kind: TokenKind::False,
                span,
            } => Expression::Boolean(false, span),
            Token {
                kind: TokenKind::If,
                span,
            } => self.if_expression(span.start)?,
            Token {
                kind: TokenKind::Ident(name),
                span,
            } if matches!(self.peek().kind, TokenKind::LParen) => {
                self.next();
                let mut arguments = Vec::new();
                let mut closing_consumed = None;
                if !matches!(self.peek().kind, TokenKind::RParen) {
                    loop {
                        let argument_start = self.at;
                        match self.expression(0) {
                            Ok(argument) => arguments.push(argument),
                            Err(diagnostic) if self.recovering => {
                                let token_consumed = self
                                    .diagnostic_token_was_consumed(argument_start, diagnostic.span);
                                let consumed =
                                    self.source.get(diagnostic.span.start..diagnostic.span.end);
                                let error_end = diagnostic.span.end;
                                self.recovery_diagnostics.push(diagnostic);
                                if token_consumed && consumed == Some(")") {
                                    closing_consumed = Some(error_end);
                                    break;
                                }
                                if !token_consumed || consumed != Some(",") {
                                    self.synchronize_expression_list(TokenKind::RParen);
                                }
                                if matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
                                    break;
                                }
                                continue;
                            }
                            Err(diagnostic) => return Err(diagnostic),
                        }
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else if self.recovering && self.starts_expression() {
                            self.recovery_diagnostics
                                .push(self.error("expected `,` between function arguments"));
                        } else {
                            break;
                        }
                    }
                }
                let end = if let Some(end) = closing_consumed {
                    end
                } else {
                    self.expect(
                        |kind| matches!(kind, TokenKind::RParen),
                        "expected `)` after arguments",
                    )?
                    .span
                    .end
                };
                Expression::Call {
                    name,
                    arguments,
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } if matches!(self.peek().kind, TokenKind::LBrace)
                && matches!(
                    self.tokens.get(self.at + 1).map(|token| &token.kind),
                    Some(TokenKind::Ident(_))
                )
                && matches!(
                    self.tokens.get(self.at + 2).map(|token| &token.kind),
                    Some(TokenKind::Colon)
                ) =>
            {
                self.next();
                let mut fields = Vec::new();
                let mut closing_consumed = None;
                if !matches!(self.peek().kind, TokenKind::RBrace) {
                    loop {
                        let field_start = self.at;
                        let field: Result<(String, Expression, Span), Diagnostic> = (|| {
                            let (field_name, field_span) =
                                self.ident("expected field name in structure literal")?;
                            self.expect(
                                |kind| matches!(kind, TokenKind::Colon),
                                "expected `:` after structure literal field",
                            )?;
                            let value = self.expression(0)?;
                            Ok((field_name, value, field_span))
                        })(
                        );
                        match field {
                            Ok(field) => fields.push(field),
                            Err(diagnostic) if self.recovering => {
                                let token_consumed = self
                                    .diagnostic_token_was_consumed(field_start, diagnostic.span);
                                let consumed =
                                    self.source.get(diagnostic.span.start..diagnostic.span.end);
                                let error_end = diagnostic.span.end;
                                self.recovery_diagnostics.push(diagnostic);
                                if token_consumed && consumed == Some("}") {
                                    closing_consumed = Some(error_end);
                                    break;
                                }
                                if !token_consumed || consumed != Some(",") {
                                    self.synchronize_expression_list(TokenKind::RBrace);
                                }
                                if matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
                                    break;
                                }
                                continue;
                            }
                            Err(diagnostic) => return Err(diagnostic),
                        }
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                            if matches!(self.peek().kind, TokenKind::RBrace) {
                                break;
                            }
                        } else if self.recovering
                            && matches!(self.peek().kind, TokenKind::Ident(_))
                            && matches!(
                                self.tokens.get(self.at + 1).map(|token| &token.kind),
                                Some(TokenKind::Colon)
                            )
                        {
                            self.recovery_diagnostics
                                .push(self.error("expected `,` between structure literal fields"));
                        } else {
                            break;
                        }
                    }
                }
                let end = if let Some(end) = closing_consumed {
                    end
                } else {
                    self.expect(
                        |kind| matches!(kind, TokenKind::RBrace),
                        "expected `}` after structure literal",
                    )?
                    .span
                    .end
                };
                Expression::StructLiteral {
                    name,
                    fields,
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } => Expression::Name(name, span),
            Token {
                kind: TokenKind::Minus,
                span,
            } => {
                let inner = self.expression(10)?;
                let end = inner.span().end;
                Expression::Negate(
                    Box::new(inner),
                    Span {
                        start: span.start,
                        end,
                    },
                )
            }
            Token {
                kind: TokenKind::Bang,
                span,
            } => {
                let inner = self.expression(10)?;
                let end = inner.span().end;
                Expression::Not(
                    Box::new(inner),
                    Span {
                        start: span.start,
                        end,
                    },
                )
            }
            Token {
                kind: TokenKind::Tilde,
                span,
            } => {
                let inner = self.expression(10)?;
                let end = inner.span().end;
                Expression::BitNot(
                    Box::new(inner),
                    Span {
                        start: span.start,
                        end,
                    },
                )
            }
            Token {
                kind: TokenKind::LParen,
                ..
            } => {
                let expr = self.expression(0)?;
                self.expect(
                    |k| matches!(k, TokenKind::RParen),
                    "expected `)` after expression",
                )?;
                expr
            }
            token => {
                return Err(Diagnostic {
                    code: "R0012",
                    message: "expected expression".into(),
                    span: token.span,
                    help: Some(
                        "start with a literal, variable, function call, unary operator, or `(`."
                            .into(),
                    ),
                });
            }
        };
        let mut postfix_depth = 0;
        loop {
            if matches!(self.peek().kind, TokenKind::Dot) {
                if postfix_depth >= MAX_EXPRESSION_DEPTH {
                    return Err(self.nesting_error("field access"));
                }
                postfix_depth += 1;
                self.next();
                let (name, field_span) = self.ident("expected field name after `.`")?;
                let span = Span {
                    start: left.span().start,
                    end: field_span.end,
                };
                left = Expression::Field {
                    value: Box::new(left),
                    name,
                    name_span: field_span,
                    span,
                };
                continue;
            }
            if matches!(self.peek().kind, TokenKind::As) {
                if postfix_depth >= MAX_EXPRESSION_DEPTH {
                    return Err(self.nesting_error("cast"));
                }
                postfix_depth += 1;
                self.next();
                let target = self.type_name()?;
                let span = Span {
                    start: left.span().start,
                    end: self.tokens[self.at - 1].span.end,
                };
                left = Expression::Cast(Box::new(left), target, span);
                continue;
            }
            let (op, precedence) = match self.peek().kind {
                TokenKind::OrOr => (BinaryOp::Or, 1),
                TokenKind::AndAnd => (BinaryOp::And, 2),
                TokenKind::EqualEqual => (BinaryOp::Eq, 3),
                TokenKind::BangEqual => (BinaryOp::Ne, 3),
                TokenKind::Less => (BinaryOp::Lt, 3),
                TokenKind::LessEqual => (BinaryOp::Le, 3),
                TokenKind::Greater => (BinaryOp::Gt, 3),
                TokenKind::GreaterEqual => (BinaryOp::Ge, 3),
                TokenKind::BitOr => (BinaryOp::BitOr, 4),
                TokenKind::Caret => (BinaryOp::BitXor, 5),
                TokenKind::BitAnd => (BinaryOp::BitAnd, 6),
                TokenKind::ShiftLeft => (BinaryOp::ShiftLeft, 7),
                TokenKind::ShiftRight => (BinaryOp::ShiftRight, 7),
                TokenKind::Plus => (BinaryOp::Add, 8),
                TokenKind::Minus => (BinaryOp::Sub, 8),
                TokenKind::Star => (BinaryOp::Mul, 9),
                TokenKind::Slash => (BinaryOp::Div, 9),
                TokenKind::Percent => (BinaryOp::Rem, 9),
                _ => break,
            };
            if precedence < min_precedence {
                break;
            }
            self.next();
            let right = self.expression(precedence + 1)?;
            let span = Span {
                start: left.span().start,
                end: right.span().end,
            };
            left = Expression::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn if_expression(&mut self, start: usize) -> Result<Expression, Diagnostic> {
        let condition = self.expression(0)?;
        let then_value = self.expression_block("expected `{` after if-expression condition")?;
        self.expect(
            |kind| matches!(kind, TokenKind::Else),
            "if expression requires an `else` branch",
        )?;
        let else_value = if matches!(self.peek().kind, TokenKind::If) {
            self.expression(10)?
        } else {
            self.expression_block("expected `{` after `else`")?
        };
        let end = self.tokens[self.at - 1].span.end;
        Ok(Expression::If {
            condition: Box::new(condition),
            then_value: Box::new(then_value),
            else_value: Box::new(else_value),
            span: Span { start, end },
        })
    }

    fn expression_block(&mut self, opening_message: &str) -> Result<Expression, Diagnostic> {
        self.expect(|kind| matches!(kind, TokenKind::LBrace), opening_message)?;
        let value = self.expression(0)?;
        self.expect(
            |kind| matches!(kind, TokenKind::RBrace),
            "expected `}` after if-expression value",
        )?;
        Ok(value)
    }

    fn if_expression_reaches_function_end(&mut self) -> bool {
        let start = self.at;
        self.speculative = true;
        let reaches_end =
            self.expression(0).is_ok() && matches!(self.peek().kind, TokenKind::RBrace);
        self.at = start;
        self.speculative = false;
        reaches_end
    }

    fn nesting_error(&self, kind: &str) -> Diagnostic {
        Diagnostic {
            code: "R0015",
            message: format!("maximum parser nesting depth exceeded in {kind}"),
            span: self.peek().span,
            help: Some(format!("reduce the amount of nesting in {kind}s")),
        }
    }

    fn ident(&mut self, message: &str) -> Result<(String, Span), Diagnostic> {
        if let TokenKind::Ident(name) = &self.peek().kind {
            let name = name.clone();
            let span = self.next().span;
            Ok((name, span))
        } else {
            Err(self.error(message))
        }
    }
    fn expect(
        &mut self,
        test: impl FnOnce(&TokenKind) -> bool,
        message: &str,
    ) -> Result<Token, Diagnostic> {
        if test(&self.peek().kind) {
            Ok(self.next())
        } else {
            Err(self.error(message))
        }
    }
    fn peek(&self) -> &Token {
        &self.tokens[self.at]
    }
    fn next(&mut self) -> Token {
        if matches!(self.peek().kind, TokenKind::Eof) {
            return self.peek().clone();
        }
        if self.speculative {
            let token = self.peek().clone();
            self.at += 1;
            return token;
        }
        if self.recovering {
            let marker = match self.peek().kind {
                TokenKind::If => Some(ControlMarker::If),
                TokenKind::Else => Some(ControlMarker::Else),
                TokenKind::LBrace => Some(ControlMarker::LBrace),
                _ => None,
            };
            if let Some(marker) = marker {
                self.control_history.push((self.at, marker));
            }
        }
        let span = self.tokens[self.at].span;
        let token = std::mem::replace(
            &mut self.tokens[self.at],
            Token {
                kind: TokenKind::Eof,
                span,
            },
        );
        self.at += 1;
        token
    }
    fn error(&self, message: &str) -> Diagnostic {
        Diagnostic {
            code: "R0010",
            message: message.into(),
            span: self.peek().span,
            help: None,
        }
    }
}

fn mapped_span(offsets: &[usize], fallback: usize, start: usize, end: usize) -> Span {
    Span {
        start: offsets.get(start).copied().unwrap_or(fallback),
        end: offsets.get(end).copied().unwrap_or(fallback),
    }
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_recovering};
    use crate::ast::{Expression, Statement};

    #[test]
    fn parses_variables_and_precedence() {
        let program = parse("fn main() { let answer = 20 + 2 * 11 print(answer) }")
            .expect("valid source parses");
        assert!(matches!(
            &program.functions[0].body[0],
            Statement::Let {
                value: Expression::Binary { .. },
                ..
            }
        ));
    }

    #[test]
    fn bitwise_precedence_sits_between_arithmetic_comparison_and_logic() {
        let program = parse("fn main() { print(1 | 2 ^ 3 & 4 == 5 && true || false) }")
            .expect("bitwise expression parses");
        let Statement::Print(expression, _) = &program.functions[0].body[0] else {
            panic!("expected print statement");
        };
        let Expression::Binary {
            op: crate::ast::BinaryOp::Or,
            left: logical_left,
            ..
        } = expression
        else {
            panic!("logical OR should bind loosest");
        };
        let Expression::Binary {
            op: crate::ast::BinaryOp::And,
            left: comparison,
            ..
        } = logical_left.as_ref()
        else {
            panic!("logical AND should bind more tightly than logical OR");
        };
        let Expression::Binary {
            op: crate::ast::BinaryOp::Eq,
            left: bitwise_or,
            ..
        } = comparison.as_ref()
        else {
            panic!("comparison should bind less tightly than bitwise OR");
        };
        let Expression::Binary {
            op: crate::ast::BinaryOp::BitOr,
            left: bitwise_left,
            right: bitwise_xor,
            ..
        } = bitwise_or.as_ref()
        else {
            panic!("bitwise OR should bind more tightly than comparison");
        };
        assert!(matches!(bitwise_left.as_ref(), Expression::Integer(1, _)));
        let Expression::Binary {
            op: crate::ast::BinaryOp::BitXor,
            left: xor_left,
            right: bitwise_and,
            ..
        } = bitwise_xor.as_ref()
        else {
            panic!("bitwise XOR should bind more tightly than bitwise OR");
        };
        assert!(matches!(xor_left.as_ref(), Expression::Integer(2, _)));
        let Expression::Binary {
            op: crate::ast::BinaryOp::BitAnd,
            left: and_left,
            right: and_right,
            ..
        } = bitwise_and.as_ref()
        else {
            panic!("bitwise AND should bind most tightly among bitwise operators");
        };
        assert!(matches!(and_left.as_ref(), Expression::Integer(3, _)));
        assert!(matches!(and_right.as_ref(), Expression::Integer(4, _)));
    }

    #[test]
    fn parser_recovery_reports_independent_top_level_syntax_errors() {
        let source = "fn first() { let = 1 }\nfn second( { }\nfn main() {}";
        let diagnostics =
            parse_recovering(source).expect_err("both malformed declarations should be reported");

        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].message.contains("expected variable name"));
        assert!(diagnostics[1].message.contains("expected parameter name"));
        assert!(parse("fn main() {}").is_ok());
    }

    #[test]
    fn parser_recovery_reports_independent_function_parameter_errors() {
        let source = "fn broken(a i32, : bool, c: ) -> i32 { print(1 + ) 1 } fn main() {}";
        let diagnostics = parse_recovering(source)
            .expect_err("parameter and body syntax errors should all be reported");

        assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
        assert!(
            diagnostics[0]
                .message
                .contains("expected `:` after parameter name")
        );
        assert!(diagnostics[1].message.contains("expected parameter name"));
        assert!(diagnostics[2].message.contains("expected type name"));
        assert_eq!(
            diagnostics[3].message, "expected expression",
            "{diagnostics:?}"
        );
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
        assert!(parse("fn broken(value: i32,) { }").is_err());

        let missing_comma = parse_recovering("fn broken(first: i32 second: i32) { }")
            .expect_err("function parameters still require commas");
        assert_eq!(missing_comma.len(), 1);
        assert!(missing_comma[0].message.contains("expected `,`"));
    }

    #[test]
    fn parser_recovery_reports_independent_function_argument_errors() {
        let source = "fn main() { print(combine(1 + , 2 + , 3)) let = 4 } fn combine(a: i32, b: i32, c: i32) {}";
        let diagnostics =
            parse_recovering(source).expect_err("each malformed call argument should be reported");

        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message == "expected expression")
                .count(),
            2
        );
        assert!(diagnostics[2].message.contains("expected variable name"));
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
        assert!(parse("fn main() { combine(1,) }").is_err());

        let malformed_if = parse_recovering("fn main() { combine(if true { 1 }, 2) }")
            .expect_err("a missing branch should be diagnosed without losing the next argument");
        assert_eq!(malformed_if.len(), 1, "{malformed_if:?}");
        assert!(malformed_if[0].message.contains("requires an `else`"));

        let missing_comma = parse_recovering("fn main() { combine(1 2) }")
            .expect_err("a missing argument separator should be diagnosed");
        assert_eq!(missing_comma.len(), 1, "{missing_comma:?}");
        assert!(missing_comma[0].message.contains("expected `,`"));
        assert!(parse("fn main() { combine(1 2) }").is_err());
    }

    #[test]
    fn parser_recovery_reports_independent_structure_literal_value_errors() {
        let source = "struct Pair { a: i32, b: i32, c: i32 } fn main() { let value = Pair { a: 1 + , b: 2 + , c: } let = 3 }";
        let diagnostics = parse_recovering(source)
            .expect_err("structure literal field errors should be collected together");

        assert_eq!(diagnostics.len(), 4, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message == "expected expression")
                .count(),
            3
        );
        assert!(diagnostics[3].message.contains("expected variable name"));

        let malformed_if = parse_recovering(
            "struct Pair { a: i32, b: i32 } fn main() { let pair = Pair { a: if true { 1 }, b: 2 } }",
        )
        .expect_err("a missing branch should not hide later structure fields");
        assert_eq!(malformed_if.len(), 1, "{malformed_if:?}");
        assert!(malformed_if[0].message.contains("requires an `else`"));

        let missing_comma = parse_recovering(
            "struct Pair { a: i32, b: i32 } fn main() { let pair = Pair { a: 1 b: 2 } }",
        )
        .expect_err("a missing structure literal separator should be diagnosed");
        assert_eq!(missing_comma.len(), 1, "{missing_comma:?}");
        assert!(missing_comma[0].message.contains("expected `,`"));
        assert!(
            parse("struct Pair { a: i32 } fn main() { let pair = Pair { a: 1 b: 2 } }").is_err()
        );
    }

    #[test]
    fn parser_recovery_reports_independent_structure_field_syntax_errors() {
        let source =
            "struct Config { good: i32, broken: , next: str, : bool, last: f64 } fn main() {}";
        let diagnostics = parse_recovering(source)
            .expect_err("both malformed structure fields should be reported");

        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("expected type name"));
        assert!(diagnostics[1].message.contains("expected field name"));
        assert!(diagnostics[0].span.start < diagnostics[1].span.start);
    }

    #[test]
    fn parser_recovery_reports_independent_errors_inside_nested_blocks() {
        let source = "fn main() {\n    let = 1\n    if true {\n        print(1 + )\n        let = 2\n        print(3)\n    }\n    let = 4\n}\nfn other() { print(5 + ) }";
        let diagnostics = parse_recovering(source)
            .expect_err("all independent statement errors should be reported");

        assert_eq!(diagnostics.len(), 5, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("expected variable name"))
                .count(),
            3
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message == "expected expression")
                .count(),
            2
        );
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
    }

    #[test]
    fn parser_recovery_checks_if_and_while_bodies_after_bad_conditions() {
        let source = "fn main() { if ) { let = 1 print(2 + ) } while { let = 3 } }";
        let diagnostics = parse_recovering(source)
            .expect_err("condition errors should not hide independent body errors");

        assert_eq!(diagnostics.len(), 5, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message == "expected expression")
                .count(),
            3
        );
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("expected variable name"))
                .count(),
            2
        );
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
    }

    #[test]
    fn parser_recovery_checks_for_body_after_a_malformed_range_header() {
        let source = "fn main() { for index in 0.. { let = 1 print(2 + ) } print(3) }";
        let diagnostics = parse_recovering(source)
            .expect_err("a malformed range end should not hide loop-body errors");

        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message == "expected expression")
                .count(),
            2
        );
        assert!(diagnostics[1].message.contains("expected variable name"));
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].span.start <= pair[1].span.start)
        );
    }

    #[test]
    fn parser_recovery_checks_for_body_after_each_malformed_header_section() {
        for source in [
            "fn main() { for in 0..3 { let = 1 print(2 + ) } }",
            "fn main() { for index 0..3 { let = 1 print(2 + ) } }",
            "fn main() { for index in ..3 { let = 1 print(2 + ) } }",
            "fn main() { for index in 0 3 { let = 1 print(2 + ) } }",
        ] {
            let diagnostics = parse_recovering(source)
                .expect_err("malformed range headers should still check the loop body");

            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("expected variable name after `let`")),
                "{source}: {diagnostics:?}"
            );
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message == "expected expression"),
                "{source}: {diagnostics:?}"
            );
            assert!(
                diagnostics
                    .windows(2)
                    .all(|pair| pair[0].span.start <= pair[1].span.start),
                "{source}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn parser_recovery_skips_if_expression_else_blocks_before_control_bodies() {
        for source in [
            "fn main() { for index in 0.. if true { 1 + } else { 2 } { let = 3 } }",
            "fn main() { while if true { 1 + } else { true } { let = 4 } }",
            "fn main() { for index in 0.. if ) { 1 } else { 2 } { let = 3 } }",
            "fn main() { while if ) { true } else { true } { let = 4 } }",
            "struct Point { x: i32 } fn main() { for index in 0.. if Point { x: 1 + } == Point { x: 0 } { 1 } else { 2 } { let = 3 } }",
            "struct Point { x: i32 } fn main() { while if Point { x: 1 + } == Point { x: 0 } { true } else { false } { let = 4 } }",
        ] {
            let diagnostics = parse_recovering(source)
                .expect_err("malformed if expressions and loop bodies should be reported");

            assert_eq!(diagnostics.len(), 2, "{source}: {diagnostics:?}");
            assert_eq!(diagnostics[0].message, "expected expression", "{source}");
            assert!(
                diagnostics[1].message.contains("expected variable name"),
                "{source}: {diagnostics:?}"
            );
        }
    }

    #[test]
    fn parser_recovery_returns_lexical_errors_without_cascading_parse_errors() {
        let diagnostics = parse_recovering("@ fn main() { 💥 }")
            .expect_err("both lexical errors should be reported");

        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == "R0005")
        );
    }

    #[test]
    fn parses_if_expressions_and_else_if_chains() {
        let program =
            parse("fn main() { let answer = if true { 1 } else if false { 2 } else { 3 } }")
                .expect("if expression parses");
        let Statement::Let {
            value: Expression::If {
                else_value, span, ..
            },
            ..
        } = &program.functions[0].body[0]
        else {
            panic!("let initializer should be an if expression");
        };

        assert!(matches!(**else_value, Expression::If { .. }));
        assert_eq!(
            &"fn main() { let answer = if true { 1 } else if false { 2 } else { 3 } }"
                [span.start..span.end],
            "if true { 1 } else if false { 2 } else { 3 }"
        );
    }

    #[test]
    fn requires_if_expression_branches_to_be_value_blocks_with_an_else() {
        let missing_else = parse("fn main() { let answer = if true { 1 } }")
            .expect_err("if expression without else is incomplete");
        assert_eq!(
            missing_else.message,
            "if expression requires an `else` branch"
        );

        let statement_branch =
            parse("fn main() { let answer = if true { let value = 1 } else { 2 } }")
                .expect_err("if expression branch must produce a value");
        assert_eq!(statement_branch.message, "expected expression");
    }

    #[test]
    fn deeply_nested_if_expressions_return_a_diagnostic_without_panicking() {
        let mut expression = "0".to_string();
        for _ in 0..=super::MAX_EXPRESSION_DEPTH {
            expression = format!("if true {{ {expression} }} else {{ 0 }}");
        }
        let source = format!("fn main() {{ let value = {expression} }}");

        let diagnostic = parse(&source).expect_err("excessive expression nesting is rejected");
        assert_eq!(diagnostic.code, "R0015");
        assert!(diagnostic.message.contains("maximum parser nesting depth"));
    }

    #[test]
    fn rejects_missing_closing_brace() {
        assert!(parse("fn main() { print(\"Hi\")").is_err());
    }

    #[test]
    fn malformed_print_interpolation_has_a_parser_diagnostic() {
        let source = "fn main() { print(\"Hello, {name\") }";
        let diagnostic = parse(source).expect_err("unterminated interpolation is invalid");
        assert_eq!(diagnostic.code, "R0014");
        assert_eq!(diagnostic.message, "unterminated print interpolation");
        assert_eq!(
            diagnostic.span.start,
            source.find("{name").expect("opening brace is present")
        );
    }

    #[test]
    fn deeply_nested_expressions_return_a_diagnostic_without_panicking() {
        let source = format!(
            "fn main() {{ print({}true{}) }}",
            "(".repeat(super::MAX_EXPRESSION_DEPTH + 1),
            ")".repeat(super::MAX_EXPRESSION_DEPTH + 1),
        );
        let result = std::panic::catch_unwind(|| parse(&source));
        let diagnostic = result
            .expect("deeply nested input must not panic")
            .expect_err("excessive nesting must be rejected");

        assert_eq!(diagnostic.code, "R0015");
        assert!(diagnostic.message.contains("expression"));
    }

    #[test]
    fn expression_nesting_at_the_limit_is_accepted() {
        let nesting = super::MAX_EXPRESSION_DEPTH - 1;
        let source = format!(
            "fn main() {{ print({}true{}) }}",
            "(".repeat(nesting),
            ")".repeat(nesting),
        );

        parse(&source).expect("nesting at the supported limit must parse");
    }

    #[test]
    fn deeply_nested_blocks_return_a_diagnostic_without_panicking() {
        let source = format!(
            "fn main() {{ {}print(true){} }}",
            "while true {".repeat(super::MAX_BLOCK_DEPTH + 1),
            "}".repeat(super::MAX_BLOCK_DEPTH + 1),
        );
        let result = std::panic::catch_unwind(|| parse(&source));
        let diagnostic = result
            .expect("deeply nested input must not panic")
            .expect_err("excessive nesting must be rejected");

        assert_eq!(diagnostic.code, "R0015");
        assert!(diagnostic.message.contains("block"));
    }

    #[test]
    fn deeply_chained_else_if_returns_a_diagnostic_without_panicking() {
        let source = format!(
            "fn main() {{ if false {{}}{} else {{}} }}",
            " else if false {}".repeat(super::MAX_IF_DEPTH),
        );
        let result = std::panic::catch_unwind(|| parse(&source));
        let diagnostic = result
            .expect("deeply chained conditionals must not panic")
            .expect_err("excessive conditional nesting must be rejected");

        assert_eq!(diagnostic.code, "R0015");
        assert!(diagnostic.message.contains("conditional"));
    }

    #[test]
    fn else_if_chain_at_the_supported_limit_is_accepted() {
        let source = format!(
            "fn main() {{ if false {{}}{} else {{}} }}",
            " else if false {}".repeat(super::MAX_IF_DEPTH - 1),
        );

        parse(&source).expect("conditional nesting at the supported limit must parse");
    }

    #[test]
    fn block_nesting_at_the_limit_is_accepted() {
        let source = format!(
            "fn main() {{ {}print(true){} }}",
            "while true {".repeat(super::MAX_BLOCK_DEPTH),
            "}".repeat(super::MAX_BLOCK_DEPTH),
        );

        parse(&source).expect("nesting at the supported limit must parse");
    }

    #[test]
    fn reports_a_missing_function_name_at_eof_without_panicking() {
        let diagnostic = parse("fn").expect_err("function name is missing");
        assert_eq!(diagnostic.code, "R0010");
        assert_eq!(diagnostic.message, "expected function name");
    }

    #[test]
    fn parser_does_not_panic_on_truncated_program_prefixes() {
        let examples = [
            ("conditions", include_str!("../examples/conditions.ryn")),
            (
                "evaluation_order",
                include_str!("../examples/evaluation_order.ryn"),
            ),
            ("floats", include_str!("../examples/floats.ryn")),
            (
                "function_statements",
                include_str!("../examples/function_statements.ryn"),
            ),
            ("functions", include_str!("../examples/functions.ryn")),
            ("hello", include_str!("../examples/hello.ryn")),
            ("i32", include_str!("../examples/i32.ryn")),
            (
                "integer_widths",
                include_str!("../examples/integer_widths.ryn"),
            ),
            ("loops", include_str!("../examples/loops.ryn")),
            ("string_abi", include_str!("../examples/string_abi.ryn")),
            ("structs", include_str!("../examples/structs.ryn")),
            ("variables", include_str!("../examples/variables.ryn")),
        ];

        for (name, source) in examples {
            for end in source
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(source.len()))
            {
                assert!(
                    std::panic::catch_unwind(|| parse(&source[..end])).is_ok(),
                    "parser panicked for `{name}` prefix ending at byte {end}"
                );
            }
        }
    }

    #[test]
    fn parser_does_not_panic_on_short_malformed_token_sequences() {
        let fragments = [
            "fn",
            "struct",
            "name",
            "(",
            ")",
            "{",
            "}",
            "let",
            "mut",
            ":",
            "=",
            "->",
            "return",
            "if",
            "else",
            "while",
            "print",
            "true",
            "1",
            "1.0",
            "+",
            "!",
            "&&",
            ",",
            ".",
            "\"x\"",
            "@",
            "// comment",
        ];

        for first in fragments {
            assert!(
                std::panic::catch_unwind(|| parse(first)).is_ok(),
                "input: {first:?}"
            );
            for second in fragments {
                let source = format!("{first} {second}");
                assert!(
                    std::panic::catch_unwind(|| parse(&source)).is_ok(),
                    "input: {source:?}"
                );
                for third in fragments {
                    let source = format!("{first} {second} {third}");
                    assert!(
                        std::panic::catch_unwind(|| parse(&source)).is_ok(),
                        "input: {source:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn parses_explicit_return_as_a_statement() {
        let program =
            parse("fn answer() -> i32 { return 42 }").expect("explicit return statement parses");

        assert!(matches!(
            program.functions[0].body[0],
            Statement::Return { .. }
        ));
    }

    #[test]
    fn field_access_chains_obey_the_expression_depth_limit() {
        let path = format!("value{}", ".field".repeat(super::MAX_EXPRESSION_DEPTH + 1));
        let error = parse(&format!("fn main() {{ print({path}) }}"))
            .expect_err("excessive field access nesting is rejected");
        assert_eq!(error.code, "R0015");
        assert!(error.message.contains("field access"));

        let assignment = format!(
            "value{} = 1",
            ".field".repeat(super::MAX_EXPRESSION_DEPTH + 1)
        );
        let error = parse(&format!("fn main() {{ {assignment} }}"))
            .expect_err("excessive field assignment nesting is rejected");
        assert_eq!(error.code, "R0015");
    }
}
