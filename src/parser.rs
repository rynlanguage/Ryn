use std::collections::{HashMap, HashSet};

use crate::{
    ast::{
        BinaryOp, ChooseArm, EnumDef, Expression, Function, Parameter, PrintPart, Program,
        Statement, StructDef, StructField, TypeAliasDef, TypeName, UseDecl, VariantDef,
    },
    lexer::{Token, TokenKind, lex, lex_recovering},
    source::{Diagnostic, Span},
};

// Keep recursive expression parsing comfortably below the default Windows
// process-thread stack limit while still allowing ordinary deep expressions.
const MAX_EXPRESSION_DEPTH: usize = 24;
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
    type_aliases: HashMap<String, TypeName>,
    generic_type_aliases: HashMap<String, (Vec<String>, TypeName)>,
    tuple_type_names: HashMap<String, String>,
    tuple_type_structs: Vec<StructDef>,
    generic_struct_templates: HashMap<String, StructDef>,
    generic_struct_keys: HashMap<String, String>,
    generic_struct_placeholder_keys: HashMap<String, String>,
    generic_struct_placeholders: HashMap<String, (String, Vec<TypeName>)>,
    generic_structs: Vec<StructDef>,
    generic_enum_keys: HashMap<String, String>,
    generic_enum_placeholder_keys: HashMap<String, String>,
    generic_enum_placeholders: HashMap<String, (String, Vec<TypeName>)>,
    generic_enums: Vec<EnumDef>,
    generic_enum_templates: HashMap<String, EnumDef>,
    namespace_path: Vec<String>,
    generic_type_parameters: HashSet<String>,
    custom_drop_types: HashSet<String>,
}

#[derive(Clone, Copy)]
enum ControlMarker {
    If,
    Else,
    LBrace,
}

enum ForHeader {
    Range(Expression, Expression),
    Each(Expression),
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
        let mut custom_drop_types = HashSet::new();
        for (index, token) in tokens.iter().enumerate() {
            if !matches!(token.kind, TokenKind::Hash)
                || !matches!(tokens.get(index + 2).map(|token| &token.kind), Some(TokenKind::Ident(name)) if name == "drop")
            {
                continue;
            }
            let Some(attribute_end) = (index + 3..tokens.len())
                .find(|candidate| matches!(tokens[*candidate].kind, TokenKind::RBracket))
            else {
                continue;
            };
            if matches!(
                tokens.get(attribute_end + 1).map(|token| &token.kind),
                Some(TokenKind::Struct)
            ) && let Some(Token {
                kind: TokenKind::Ident(name),
                ..
            }) = tokens.get(attribute_end + 2)
            {
                custom_drop_types.insert(name.clone());
            }
        }
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
            type_aliases: HashMap::new(),
            generic_type_aliases: HashMap::new(),
            tuple_type_names: HashMap::new(),
            tuple_type_structs: Vec::new(),
            generic_struct_templates: HashMap::new(),
            generic_struct_keys: HashMap::new(),
            generic_struct_placeholder_keys: HashMap::new(),
            generic_struct_placeholders: HashMap::new(),
            generic_structs: Vec::new(),
            generic_enum_keys: HashMap::new(),
            generic_enum_placeholder_keys: HashMap::new(),
            generic_enum_placeholders: HashMap::new(),
            generic_enums: Vec::new(),
            generic_enum_templates: HashMap::new(),
            namespace_path: Vec::new(),
            generic_type_parameters: HashSet::new(),
            custom_drop_types,
        }
    }

    fn ensure_map_option_specialization(&mut self, value: &TypeName, span: Span) {
        let is_custom_drop = self.is_custom_drop_type(value, &mut HashSet::new());
        if !is_custom_drop {
            self.ensure_option_specialization(value, span);
        }
    }

    fn is_custom_drop_type(&self, ty: &TypeName, aliases: &mut HashSet<String>) -> bool {
        match ty {
            TypeName::Named(name, _) => {
                let leaf = name.rsplit("::").next().unwrap_or(name);
                if self.custom_drop_types.contains(name) || self.custom_drop_types.contains(leaf) {
                    return true;
                }
                if !aliases.insert(name.clone()) {
                    return false;
                }
                self.type_aliases
                    .get(name)
                    .or_else(|| self.type_aliases.get(leaf))
                    .is_some_and(|target| self.is_custom_drop_type(target, aliases))
            }
            _ => false,
        }
    }

    fn program(mut self) -> Result<Program, Diagnostic> {
        let mut uses = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut type_aliases = Vec::new();
        let mut functions = Vec::new();
        while !matches!(self.peek().kind, TokenKind::Eof) {
            if matches!(self.peek().kind, TokenKind::Namespace) {
                self.namespace_decl()?;
                continue;
            }
            let (repr_c, drop_function) = if matches!(self.peek().kind, TokenKind::Hash) {
                if self.next_attribute_is_drop() {
                    (false, Some(self.drop_attribute()?))
                } else {
                    (self.repr_c_attribute()?, None)
                }
            } else {
                (false, None)
            };
            let public = if matches!(self.peek().kind, TokenKind::Pub) {
                self.next();
                true
            } else {
                false
            };
            if (repr_c || drop_function.is_some()) && !matches!(self.peek().kind, TokenKind::Struct)
            {
                return Err(self.error("`#[repr(C)]` applies only to structs"));
            }
            if matches!(self.peek().kind, TokenKind::Use) {
                if public {
                    return Err(self.error("`pub use` is not supported"));
                }
                uses.push(self.use_decl()?);
            } else if matches!(self.peek().kind, TokenKind::Struct) {
                let definition = self.struct_def(public, repr_c, drop_function)?;
                if definition.type_parameters.is_empty() {
                    structs.push(definition);
                } else {
                    self.add_generic_struct_template(definition)?;
                }
            } else if matches!(self.peek().kind, TokenKind::Enum) {
                if repr_c {
                    return Err(self.error("`#[repr(C)]` applies only to structs"));
                }
                let definition = self.enum_def(public)?;
                if definition.type_parameters.is_empty() {
                    enums.push(definition);
                } else if self
                    .generic_enum_templates
                    .insert(definition.name.clone(), definition.clone())
                    .is_some()
                {
                    return Err(Diagnostic {
                        code: "R0221",
                        message: format!("duplicate enum `{}`", definition.name),
                        span: definition.span,
                        help: Some("give each enum a unique name".into()),
                    });
                }
            } else if matches!(self.peek().kind, TokenKind::TypeAlias) {
                if repr_c {
                    return Err(self.error("`#[repr(C)]` applies only to structs"));
                }
                type_aliases.push(self.type_alias(public)?);
            } else if matches!(&self.peek().kind, TokenKind::Ident(name) if name == "extern") {
                if repr_c {
                    return Err(self.error("`#[repr(C)]` applies only to structs"));
                }
                self.next();
                match self.next().kind {
                    TokenKind::String(abi) if abi == "C" => {}
                    _ => return Err(self.error("expected `\"C\"` after `extern`")),
                }
                functions.push(self.function(public, true)?);
            } else {
                if repr_c {
                    return Err(self.error("`#[repr(C)]` applies only to structs"));
                }
                functions.push(self.function(public, false)?);
            }
        }
        enums.extend(self.generic_enums);
        structs.extend(self.generic_structs);
        structs.extend(self.tuple_type_structs);
        Ok(Program {
            uses,
            structs,
            enums,
            type_aliases,
            functions,
        })
    }

    fn program_recovering(mut self) -> Result<Program, Vec<Diagnostic>> {
        let mut uses = Vec::new();
        let mut structs = Vec::new();
        let mut enums = Vec::new();
        let mut type_aliases = Vec::new();
        let mut functions = Vec::new();
        let mut diagnostics = Vec::new();

        while !matches!(self.peek().kind, TokenKind::Eof) {
            let declaration_start = self.at;
            if matches!(self.peek().kind, TokenKind::Namespace) {
                if let Err(diagnostic) = self.namespace_decl() {
                    diagnostics.push(diagnostic);
                    self.synchronize_top_level();
                }
                diagnostics.append(&mut self.recovery_diagnostics);
                continue;
            }
            let result = (|| {
                let (repr_c, drop_function) = if matches!(self.peek().kind, TokenKind::Hash) {
                    if self.next_attribute_is_drop() {
                        (false, Some(self.drop_attribute()?))
                    } else {
                        (self.repr_c_attribute()?, None)
                    }
                } else {
                    (false, None)
                };
                let public = if matches!(self.peek().kind, TokenKind::Pub) {
                    self.next();
                    true
                } else {
                    false
                };
                if (repr_c || drop_function.is_some())
                    && !matches!(self.peek().kind, TokenKind::Struct)
                {
                    return Err(self.error("`#[repr(C)]` applies only to structs"));
                }
                if matches!(self.peek().kind, TokenKind::Use) && !public {
                    self.use_decl().map(|declaration| uses.push(declaration))
                } else if matches!(self.peek().kind, TokenKind::Struct) {
                    self.struct_def(public, repr_c, drop_function)
                        .map(|definition| {
                            if definition.type_parameters.is_empty() {
                                structs.push(definition);
                            } else {
                                if let Err(diagnostic) =
                                    self.add_generic_struct_template(definition)
                                {
                                    self.recovery_diagnostics.push(diagnostic);
                                }
                            }
                        })
                } else if matches!(self.peek().kind, TokenKind::Enum) {
                    self.enum_def(public).map(|definition| {
                        if definition.type_parameters.is_empty() {
                            enums.push(definition);
                        } else {
                            self.generic_enum_templates
                                .insert(definition.name.clone(), definition);
                        }
                    })
                } else if matches!(self.peek().kind, TokenKind::TypeAlias) {
                    self.type_alias(public)
                        .map(|definition| type_aliases.push(definition))
                } else if matches!(&self.peek().kind, TokenKind::Ident(name) if name == "extern") {
                    self.next();
                    match self.next().kind {
                        TokenKind::String(abi) if abi == "C" => self
                            .function(public, true)
                            .map(|function| functions.push(function)),
                        _ => Err(self.error("expected `\"C\"` after `extern`")),
                    }
                } else {
                    self.function(public, false)
                        .map(|function| functions.push(function))
                }
            })();
            diagnostics.append(&mut self.recovery_diagnostics);

            match result {
                Ok(()) => {}
                Err(diagnostic) => {
                    self.generic_type_parameters.clear();
                    diagnostics.push(diagnostic);
                    if self.at == declaration_start {
                        self.next();
                    }
                    self.synchronize_top_level();
                }
            }
        }

        if diagnostics.is_empty() {
            enums.extend(self.generic_enums);
            structs.extend(self.generic_structs);
            structs.extend(self.tuple_type_structs);
            Ok(Program {
                uses,
                structs,
                enums,
                type_aliases,
                functions,
            })
        } else {
            Err(diagnostics)
        }
    }

    fn synchronize_top_level(&mut self) {
        while !matches!(
            self.peek().kind,
            TokenKind::Eof
                | TokenKind::Fn
                | TokenKind::Struct
                | TokenKind::Enum
                | TokenKind::TypeAlias
                | TokenKind::Use
                | TokenKind::Namespace
                | TokenKind::Pub
        ) {
            self.next();
        }
    }

    fn namespace_decl(&mut self) -> Result<(), Diagnostic> {
        self.expect(
            |kind| matches!(kind, TokenKind::Namespace),
            "expected `namespace`",
        )?;
        if matches!(self.peek().kind, TokenKind::Semicolon) {
            self.next();
            self.namespace_path.clear();
            return Ok(());
        }
        let (first, _) = self.ident("expected a namespace name or `;` to reset it")?;
        let mut path = vec![first];
        while matches!(self.peek().kind, TokenKind::ColonColon) {
            self.next();
            path.push(self.ident("expected namespace segment after `::`")?.0);
        }
        if matches!(self.peek().kind, TokenKind::Semicolon) {
            self.next();
        }
        self.namespace_path = path;
        Ok(())
    }

    fn qualified_declaration_name(&self, name: String) -> String {
        if self.namespace_path.is_empty() {
            name
        } else {
            format!("{}::{name}", self.namespace_path.join("::"))
        }
    }

    fn use_decl(&mut self) -> Result<UseDecl, Diagnostic> {
        let start = self
            .expect(|kind| matches!(kind, TokenKind::Use), "expected `use`")?
            .span
            .start;
        let (first, first_span) = self.ident("expected module path after `use`")?;
        let mut path = vec![first];
        let mut end = first_span.end;
        while matches!(self.peek().kind, TokenKind::ColonColon) {
            self.next();
            let (segment, segment_span) = self.ident("expected module path segment after `::`")?;
            path.push(segment);
            end = segment_span.end;
        }
        Ok(UseDecl {
            path,
            span: Span { start, end },
        })
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

    fn repr_c_attribute(&mut self) -> Result<bool, Diagnostic> {
        self.expect(|kind| matches!(kind, TokenKind::Hash), "expected `#`")?;
        self.expect(
            |kind| matches!(kind, TokenKind::LBracket),
            "expected `[` after `#`",
        )?;
        let attribute = self.next();
        if !matches!(attribute.kind, TokenKind::Ident(ref name) if name == "repr") {
            return Err(Diagnostic {
                code: "R0014",
                message: "unsupported attribute".into(),
                span: attribute.span,
                help: Some("the supported representation attribute is `#[repr(C)]`".into()),
            });
        }
        self.expect(
            |kind| matches!(kind, TokenKind::LParen),
            "expected `(` after `repr`",
        )?;
        let representation = self.next();
        if !matches!(representation.kind, TokenKind::Ident(ref name) if name == "C") {
            return Err(Diagnostic {
                code: "R0014",
                message: "unsupported structure representation".into(),
                span: representation.span,
                help: Some("the supported representation is `C`".into()),
            });
        }
        self.expect(
            |kind| matches!(kind, TokenKind::RParen),
            "expected `)` after representation",
        )?;
        self.expect(
            |kind| matches!(kind, TokenKind::RBracket),
            "expected `]` after attribute",
        )?;
        Ok(true)
    }

    fn next_attribute_is_drop(&self) -> bool {
        matches!(
            self.tokens.get(self.at + 2).map(|token| &token.kind),
            Some(TokenKind::Ident(name)) if name == "drop"
        )
    }

    fn drop_attribute(&mut self) -> Result<String, Diagnostic> {
        self.expect(|kind| matches!(kind, TokenKind::Hash), "expected `#`")?;
        self.expect(
            |kind| matches!(kind, TokenKind::LBracket),
            "expected `[` after `#`",
        )?;
        let attribute = self.next();
        if !matches!(attribute.kind, TokenKind::Ident(ref name) if name == "drop") {
            return Err(self.error("expected `drop` in destructor attribute"));
        }
        self.expect(
            |kind| matches!(kind, TokenKind::LParen),
            "expected `(` after `drop`",
        )?;
        let (mut function, _) = self.ident("expected destructor function name")?;
        while matches!(self.peek().kind, TokenKind::ColonColon) {
            self.next();
            let (segment, _) = self.ident("expected destructor path segment after `::`")?;
            function.push_str("::");
            function.push_str(&segment);
        }
        self.expect(
            |kind| matches!(kind, TokenKind::RParen),
            "expected `)` after destructor function name",
        )?;
        self.expect(
            |kind| matches!(kind, TokenKind::RBracket),
            "expected `]` after destructor attribute",
        )?;
        Ok(if function.contains("::") {
            function
        } else {
            self.qualified_declaration_name(function)
        })
    }

    fn struct_def(
        &mut self,
        public: bool,
        repr_c: bool,
        drop_function: Option<String>,
    ) -> Result<StructDef, Diagnostic> {
        let start = self.next().span.start;
        let (name, _) = self.ident("expected structure name")?;
        let name = self.qualified_declaration_name(name);
        if repr_c && matches!(self.peek().kind, TokenKind::Less) {
            return Err(self.error("generic `#[repr(C)]` structs are not supported"));
        }
        let mut type_parameters = Vec::new();
        if matches!(self.peek().kind, TokenKind::Less) {
            self.next();
            loop {
                let (parameter, span) = self.ident("expected generic type parameter")?;
                if type_parameters.contains(&parameter)
                    || self.generic_type_parameters.contains(&parameter)
                {
                    return Err(Diagnostic {
                        code: "R0242",
                        message: format!("duplicate generic type parameter `{parameter}`"),
                        span,
                        help: None,
                    });
                }
                self.generic_type_parameters.insert(parameter.clone());
                type_parameters.push(parameter);
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect_type_greater("expected `>` after generic type parameters")?;
        }
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
        for parameter in &type_parameters {
            self.generic_type_parameters.remove(parameter);
        }
        if drop_function.is_some() && !type_parameters.is_empty() {
            return Err(self.error("generic structures cannot yet define custom destructors"));
        }
        Ok(StructDef {
            name,
            type_parameters,
            fields,
            public,
            repr_c,
            drop_function,
            module_path: String::new(),
            span: Span { start, end },
        })
    }

    fn add_generic_struct_template(&mut self, definition: StructDef) -> Result<(), Diagnostic> {
        if self.generic_struct_templates.contains_key(&definition.name) {
            return Err(Diagnostic {
                code: "R0220",
                message: format!("duplicate structure `{}`", definition.name),
                span: definition.span,
                help: Some("give each structure a unique name".into()),
            });
        }
        self.generic_struct_templates
            .insert(definition.name.clone(), definition);
        Ok(())
    }

    fn enum_def(&mut self, public: bool) -> Result<EnumDef, Diagnostic> {
        let start = self.next().span.start;
        let (name, _) = self.ident("expected enum name")?;
        let name = self.qualified_declaration_name(name);
        let mut type_parameters = Vec::new();
        if matches!(self.peek().kind, TokenKind::Less) {
            self.next();
            loop {
                let (parameter, span) = self.ident("expected generic type parameter")?;
                if type_parameters.contains(&parameter)
                    || self.generic_type_parameters.contains(&parameter)
                {
                    return Err(Diagnostic {
                        code: "R0242",
                        message: format!("duplicate generic type parameter `{parameter}`"),
                        span,
                        help: None,
                    });
                }
                self.generic_type_parameters.insert(parameter.clone());
                type_parameters.push(parameter);
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect_type_greater("expected `>` after generic type parameters")?;
        }
        self.expect(
            |kind| matches!(kind, TokenKind::LBrace),
            "expected `{` after enum name",
        )?;
        let mut variants = Vec::new();
        while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
            let variant_start = self.at;
            let variant = (|| {
                let (variant_name, variant_span) = self.ident("expected variant name")?;
                let mut fields = Vec::new();
                if matches!(self.peek().kind, TokenKind::LParen) {
                    self.next();
                    if !matches!(self.peek().kind, TokenKind::RParen) {
                        loop {
                            fields.push(self.type_name()?);
                            if matches!(self.peek().kind, TokenKind::Comma) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(
                        |kind| matches!(kind, TokenKind::RParen),
                        "expected `)` after variant fields",
                    )?;
                }
                Ok(VariantDef {
                    name: variant_name,
                    fields,
                    span: variant_span,
                })
            })();
            match variant {
                Ok(variant) => variants.push(variant),
                Err(diagnostic) if self.recovering => {
                    self.recovery_diagnostics.push(diagnostic);
                    if self.at == variant_start {
                        self.next();
                    }
                    while !matches!(
                        self.peek().kind,
                        TokenKind::Comma | TokenKind::RBrace | TokenKind::Eof
                    ) {
                        self.next();
                    }
                }
                Err(diagnostic) => return Err(diagnostic),
            }
            if matches!(self.peek().kind, TokenKind::Comma) {
                self.next();
            } else if !matches!(self.peek().kind, TokenKind::RBrace) {
                break;
            }
        }
        let end = self
            .expect(
                |kind| matches!(kind, TokenKind::RBrace),
                "expected `}` after enum variants",
            )?
            .span
            .end;
        for parameter in &type_parameters {
            self.generic_type_parameters.remove(parameter);
        }
        Ok(EnumDef {
            name,
            type_parameters,
            variants,
            public,
            module_path: String::new(),
            span: Span { start, end },
        })
    }

    fn type_alias(&mut self, public: bool) -> Result<TypeAliasDef, Diagnostic> {
        let start = self.next().span.start;
        let (name, name_span) = self.ident("expected a type alias name")?;
        let name = self.qualified_declaration_name(name);
        let mut type_parameters = Vec::new();
        if matches!(self.peek().kind, TokenKind::Less) {
            self.next();
            loop {
                let (parameter, span) = self.ident("expected generic type parameter")?;
                if type_parameters.contains(&parameter)
                    || self.generic_type_parameters.contains(&parameter)
                {
                    return Err(Diagnostic {
                        code: "R0242",
                        message: format!("duplicate generic type parameter `{parameter}`"),
                        span,
                        help: None,
                    });
                }
                self.generic_type_parameters.insert(parameter.clone());
                type_parameters.push(parameter);
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect_type_greater("expected `>` after generic type parameters")?;
        }
        self.expect(
            |kind| matches!(kind, TokenKind::Equal),
            "expected `=` after type alias name",
        )?;
        let ty = self.type_name()?;
        let end = self.tokens[self.at - 1].span.end;
        for parameter in &type_parameters {
            self.generic_type_parameters.remove(parameter);
        }
        if self.type_aliases.contains_key(&name) || self.generic_type_aliases.contains_key(&name) {
            return Err(Diagnostic {
                code: "R0241",
                message: format!("duplicate type alias `{name}`"),
                span: name_span,
                help: Some("give each type alias a unique name".into()),
            });
        }
        if type_parameters.is_empty() {
            self.type_aliases.insert(name.clone(), ty.clone());
        } else {
            self.generic_type_aliases
                .insert(name.clone(), (type_parameters.clone(), ty.clone()));
        }
        Ok(TypeAliasDef {
            name,
            type_parameters,
            ty,
            public,
            module_path: String::new(),
            span: Span { start, end },
        })
    }

    fn function(&mut self, public: bool, extern_c: bool) -> Result<Function, Diagnostic> {
        let start = self
            .expect(|k| matches!(k, TokenKind::Fn), "expected `fun`")?
            .span
            .start;
        let name = match self.next().kind {
            TokenKind::Ident(name) => name,
            _ => return Err(self.error("expected function name")),
        };
        let external_symbol = extern_c.then(|| name.clone());
        let name = self.qualified_declaration_name(name);
        let mut type_parameters = Vec::new();
        if matches!(self.peek().kind, TokenKind::Less) {
            self.next();
            loop {
                let (parameter, parameter_span) = self.ident("expected generic type parameter")?;
                if self.generic_type_parameters.contains(&parameter)
                    || type_parameters.contains(&parameter)
                    || is_builtin_type_name(&parameter)
                {
                    return Err(Diagnostic {
                        code: "R0260",
                        message: format!("invalid or duplicate generic parameter `{parameter}`"),
                        span: parameter_span,
                        help: Some("choose a unique non-builtin type parameter name".into()),
                    });
                }
                self.generic_type_parameters.insert(parameter.clone());
                type_parameters.push(parameter);
                if matches!(self.peek().kind, TokenKind::Comma) {
                    self.next();
                    continue;
                }
                self.expect_type_greater("expected `>` after generic type parameters")?;
                break;
            }
        }
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
        if extern_c {
            if !type_parameters.is_empty() {
                return Err(self.error("generic `extern \"C\"` functions are not supported"));
            }
            let end = self
                .expect(
                    |kind| matches!(kind, TokenKind::Semicolon),
                    "expected `;` after the external function signature",
                )?
                .span
                .end;
            for parameter in &type_parameters {
                self.generic_type_parameters.remove(parameter);
            }
            return Ok(Function {
                name,
                extern_c,
                external_symbol,
                type_parameters,
                public,
                module_path: String::new(),
                parameters,
                return_type,
                body: Vec::new(),
                return_value: None,
                span: Span { start, end },
            });
        }
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
                        if (return_type.is_none() || !matches!(self.peek().kind, TokenKind::RBrace))
                            && matches!(
                                expression,
                                Expression::Call { .. } | Expression::MethodCall { .. }
                            )
                        {
                            body.push(self.expression_statement(expression)?);
                            continue;
                        }
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
        for parameter in &type_parameters {
            self.generic_type_parameters.remove(parameter);
        }
        Ok(Function {
            name,
            extern_c,
            external_symbol,
            type_parameters,
            public,
            module_path: String::new(),
            parameters,
            return_type,
            body,
            return_value,
            span: Span { start, end },
        })
    }

    fn ensure_option_specialization(&mut self, value: &TypeName, span: Span) {
        let option_key = format!("Option<{}>", type_key(value));
        if self.generic_enum_keys.contains_key(&option_key) {
            return;
        }
        let internal = format!("$RynOption#{}", self.generic_enum_keys.len());
        self.generic_enum_keys.insert(option_key, internal.clone());
        self.generic_enums.push(EnumDef {
            name: internal,
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Some".into(),
                    fields: vec![value.clone()],
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
    }

    fn ensure_result_specialization(&mut self, value: TypeName, error: TypeName, span: Span) {
        let key = format!("Result<{},{}>", type_key(&value), type_key(&error));
        if self.generic_enum_keys.contains_key(&key) {
            return;
        }
        let internal = format!("$RynResult#{}", self.generic_enum_keys.len());
        self.generic_enum_keys.insert(key, internal.clone());
        self.generic_enums.push(EnumDef {
            name: internal,
            type_parameters: Vec::new(),
            variants: vec![
                VariantDef {
                    name: "Ok".into(),
                    fields: vec![value],
                    span,
                },
                VariantDef {
                    name: "Err".into(),
                    fields: vec![error],
                    span,
                },
            ],
            public: false,
            module_path: String::new(),
            span,
        });
    }

    fn type_name(&mut self) -> Result<TypeName, Diagnostic> {
        if matches!(self.peek().kind, TokenKind::Ident(ref name) if name == "extern")
            || matches!(self.peek().kind, TokenKind::Fn)
        {
            let start = self.peek().span.start;
            let extern_c = if matches!(self.peek().kind, TokenKind::Ident(ref name) if name == "extern")
            {
                self.next();
                let abi = self.next();
                if !matches!(abi.kind, TokenKind::String(ref value) if value == "C") {
                    return Err(Diagnostic {
                        code: "R0012",
                        message: "function pointer ABI must be `\"C\"`".into(),
                        span: abi.span,
                        help: Some("use `extern \"C\" fun(...) -> ...`".into()),
                    });
                }
                true
            } else {
                false
            };
            self.expect(
                |kind| matches!(kind, TokenKind::Fn),
                "expected `fun` in function pointer type",
            )?;
            self.expect(
                |kind| matches!(kind, TokenKind::LParen),
                "expected `(` after function pointer `fun`",
            )?;
            let mut parameters = Vec::new();
            while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
                parameters.push(self.type_name()?);
                if !matches!(self.peek().kind, TokenKind::Comma) {
                    break;
                }
                self.next();
            }
            self.expect(
                |kind| matches!(kind, TokenKind::RParen),
                "expected `)` after function pointer parameters",
            )?;
            let result = if matches!(self.peek().kind, TokenKind::Arrow) {
                self.next();
                Some(Box::new(self.type_name()?))
            } else {
                None
            };
            let end = self.tokens[self.at - 1].span.end;
            return Ok(TypeName::FunctionPointer(
                parameters,
                result,
                extern_c,
                Span { start, end },
            ));
        }
        if matches!(self.peek().kind, TokenKind::LParen) {
            let start = self.next().span.start;
            let first = self.type_name()?;
            if !matches!(self.peek().kind, TokenKind::Comma) {
                self.expect(
                    |kind| matches!(kind, TokenKind::RParen),
                    "expected `)` after parenthesized type",
                )?;
                return Ok(first);
            }
            let mut fields = vec![first];
            while matches!(self.peek().kind, TokenKind::Comma) {
                self.next();
                if matches!(self.peek().kind, TokenKind::RParen) {
                    break;
                }
                fields.push(self.type_name()?);
            }
            let end = self
                .expect(
                    |kind| matches!(kind, TokenKind::RParen),
                    "expected `)` after tuple type",
                )?
                .span
                .end;
            return Ok(self.register_tuple_type(fields, Span { start, end }));
        }
        if matches!(self.peek().kind, TokenKind::BitAnd) {
            let start = self.next().span.start;
            if self.consume_if(|kind| matches!(kind, TokenKind::LBracket)) {
                let element = self.type_name()?;
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RBracket),
                        "expected `]` after slice element type",
                    )?
                    .span
                    .end;
                return Ok(TypeName::Slice(Box::new(element), Span { start, end }));
            }
            let mutable = self.consume_if(|kind| matches!(kind, TokenKind::Mut));
            let element = self.type_name()?;
            let end = self.tokens[self.at - 1].span.end;
            return Ok(TypeName::Reference(
                Box::new(element),
                mutable,
                Span { start, end },
            ));
        }
        if matches!(self.peek().kind, TokenKind::Star) {
            let start = self.next().span.start;
            let element = self.type_name()?;
            let end = self.tokens[self.at - 1].span.end;
            return Ok(TypeName::RawPointer(Box::new(element), Span { start, end }));
        }
        if matches!(self.peek().kind, TokenKind::LBracket) {
            let start = self.next().span.start;
            let element = self.type_name()?;
            self.expect(
                |kind| matches!(kind, TokenKind::Semicolon),
                "expected `;` between the array element type and length",
            )?;
            let length_token = self.next();
            let TokenKind::Integer(length) = length_token.kind else {
                return Err(Diagnostic {
                    code: "R0012",
                    message: "expected a non-negative integer array length".into(),
                    span: length_token.span,
                    help: Some("array lengths must be integer literals".into()),
                });
            };
            let length = usize::try_from(length).map_err(|_| Diagnostic {
                code: "R0012",
                message: "array length exceeds the host limit".into(),
                span: length_token.span,
                help: Some("use a smaller fixed array length".into()),
            })?;
            let end = self
                .expect(
                    |kind| matches!(kind, TokenKind::RBracket),
                    "expected `]` after array type",
                )?
                .span
                .end;
            return Ok(TypeName::Array(
                Box::new(element),
                length,
                Span { start, end },
            ));
        }
        let (mut name, span) = self.ident("expected type name")?;
        while matches!(self.peek().kind, TokenKind::ColonColon) {
            self.next();
            let (segment, _) = self.ident("expected type path segment after `::`")?;
            name.push_str("::");
            name.push_str(&segment);
        }
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
            "String" => Ok(TypeName::OwnedString),
            "char" => Ok(TypeName::Char),
            "bool" => Ok(TypeName::Bool),
            "Vec" => {
                self.expect(
                    |kind| matches!(kind, TokenKind::Less),
                    "expected `<` after `Vec`",
                )?;
                let element = self.type_name()?;
                self.expect_type_greater("expected `>` after the `Vec` element type")?;
                Ok(TypeName::Vec(Box::new(element), span))
            }
            "Set" => {
                self.expect(
                    |kind| matches!(kind, TokenKind::Less),
                    "expected `<` after `Set`",
                )?;
                let key = self.type_name()?;
                self.expect_type_greater("expected `>` after the `Set` element type")?;
                let value = TypeName::Bool;
                self.ensure_map_option_specialization(&value, span);
                Ok(TypeName::Map(Box::new(key), Box::new(value), span))
            }
            "Map" | "HashMap" => {
                self.expect(
                    |kind| matches!(kind, TokenKind::Less),
                    "expected `<` after `Map`",
                )?;
                let key = self.type_name()?;
                self.expect(
                    |kind| matches!(kind, TokenKind::Comma),
                    "expected `,` between Map key and value types",
                )?;
                let value = self.type_name()?;
                self.expect_type_greater("expected `>` after Map key and value types")?;
                self.ensure_map_option_specialization(&value, span);
                Ok(TypeName::Map(Box::new(key), Box::new(value), span))
            }
            "Option" | "Result" => {
                self.expect(
                    |kind| matches!(kind, TokenKind::Less),
                    "expected `<` after `Option` or `Result`",
                )?;
                let mut arguments = vec![self.type_name()?];
                if name == "Result" {
                    self.expect(
                        |kind| matches!(kind, TokenKind::Comma),
                        "expected `,` between the `Result` value and error types",
                    )?;
                    arguments.push(self.type_name()?);
                }
                self.expect_type_greater("expected `>` after generic type arguments")?;
                let key = format!(
                    "{name}<{}>",
                    arguments.iter().map(type_key).collect::<Vec<_>>().join(",")
                );
                let internal = if let Some(internal) = self.generic_enum_keys.get(&key) {
                    internal.clone()
                } else {
                    let internal = format!("$Ryn{name}#{}", self.generic_enum_keys.len());
                    let variants = if name == "Option" {
                        vec![
                            VariantDef {
                                name: "Some".into(),
                                fields: vec![arguments[0].clone()],
                                span,
                            },
                            VariantDef {
                                name: "None".into(),
                                fields: Vec::new(),
                                span,
                            },
                        ]
                    } else {
                        vec![
                            VariantDef {
                                name: "Ok".into(),
                                fields: vec![arguments[0].clone()],
                                span,
                            },
                            VariantDef {
                                name: "Err".into(),
                                fields: vec![arguments[1].clone()],
                                span,
                            },
                        ]
                    };
                    self.generic_enum_keys.insert(key, internal.clone());
                    self.generic_enums.push(EnumDef {
                        name: internal.clone(),
                        type_parameters: Vec::new(),
                        variants,
                        public: false,
                        module_path: String::new(),
                        span,
                    });
                    internal
                };
                Ok(TypeName::Named(internal, span))
            }
            _ if self.generic_type_parameters.contains(&name) => {
                Ok(TypeName::Parameter(name, span))
            }
            _ => {
                let template_name = if name.contains("::") {
                    name.clone()
                } else if !self.namespace_path.is_empty() {
                    format!("{}::{name}", self.namespace_path.join("::"))
                } else {
                    name.clone()
                };
                if let Some((parameters, mut target)) =
                    self.generic_type_aliases.get(&template_name).cloned()
                    && matches!(self.peek().kind, TokenKind::Less)
                {
                    self.next();
                    let mut arguments = Vec::new();
                    loop {
                        arguments.push(self.type_name()?);
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    self.expect_type_greater("expected `>` after generic type alias arguments")?;
                    if parameters.len() != arguments.len() {
                        return Err(Diagnostic {
                            code: "R0245",
                            message: format!(
                                "generic type alias `{template_name}` expects {} type argument(s), found {}",
                                parameters.len(),
                                arguments.len()
                            ),
                            span,
                            help: None,
                        });
                    }
                    let substitutions = parameters
                        .into_iter()
                        .zip(arguments)
                        .collect::<HashMap<_, _>>();
                    substitute_type_parameters(&mut target, &substitutions);
                    self.resolve_generic_placeholders(&mut target, &substitutions)?;
                    return Ok(target);
                }
                if self.generic_struct_templates.contains_key(&template_name)
                    && matches!(self.peek().kind, TokenKind::Less)
                {
                    self.next();
                    let mut arguments = Vec::new();
                    loop {
                        arguments.push(self.type_name()?);
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    self.expect_type_greater("expected `>` after generic structure arguments")?;
                    let specialized =
                        self.specialize_generic_struct(&template_name, &arguments, span)?;
                    return Ok(TypeName::Named(specialized, span));
                }
                if self.generic_enum_templates.contains_key(&template_name)
                    && matches!(self.peek().kind, TokenKind::Less)
                {
                    self.next();
                    let mut arguments = Vec::new();
                    loop {
                        arguments.push(self.type_name()?);
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    self.expect_type_greater("expected `>` after generic enum arguments")?;
                    let specialized =
                        self.specialize_generic_enum(&template_name, &arguments, span)?;
                    return Ok(TypeName::Named(specialized, span));
                }
                // Module discovery parses each file before its imported declarations
                // are available. Preserve an otherwise unknown generic type application
                // through that preliminary parse; the combined project parse resolves
                // imported aliases, while sema still rejects unresolved applications.
                if matches!(self.peek().kind, TokenKind::Less) {
                    self.next();
                    let mut arguments = Vec::new();
                    loop {
                        arguments.push(self.type_name()?);
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    self.expect_type_greater("expected `>` after generic type arguments")?;
                    let arguments = arguments.iter().map(type_key).collect::<Vec<_>>().join(",");
                    return Ok(TypeName::Named(
                        format!("{template_name}<{arguments}>"),
                        span,
                    ));
                }
                let qualified = (!self.namespace_path.is_empty() && !name.contains("::"))
                    .then(|| format!("{}::{name}", self.namespace_path.join("::")));
                Ok(qualified
                    .as_ref()
                    .and_then(|qualified| self.type_aliases.get(qualified))
                    .or_else(|| self.type_aliases.get(&name))
                    .cloned()
                    .unwrap_or(TypeName::Named(name, span)))
            }
        }
    }

    fn specialize_generic_struct(
        &mut self,
        template_name: &str,
        arguments: &[TypeName],
        span: Span,
    ) -> Result<String, Diagnostic> {
        let template = self
            .generic_struct_templates
            .get(template_name)
            .cloned()
            .ok_or_else(|| self.error("unknown generic structure"))?;
        if template.type_parameters.len() != arguments.len() {
            return Err(Diagnostic {
                code: "R0243",
                message: format!(
                    "generic structure `{}` expects {} type argument(s), found {}",
                    template.name,
                    template.type_parameters.len(),
                    arguments.len()
                ),
                span,
                help: None,
            });
        }
        let key = format!(
            "{}<{}>",
            template_name,
            arguments.iter().map(type_key).collect::<Vec<_>>().join(",")
        );
        if arguments
            .iter()
            .any(|argument| self.contains_unresolved_generic_type(argument))
        {
            if let Some(name) = self.generic_struct_placeholder_keys.get(&key) {
                return Ok(name.clone());
            }
            let name = format!("$RynStructParam#{}", self.generic_struct_placeholders.len());
            self.generic_struct_placeholders
                .insert(name.clone(), (template_name.to_owned(), arguments.to_vec()));
            self.generic_struct_placeholder_keys
                .insert(key, name.clone());
            return Ok(name);
        }
        if let Some(name) = self.generic_struct_keys.get(&key) {
            return Ok(name.clone());
        }
        let internal_name = format!(
            "$RynStruct#{}",
            self.generic_struct_keys.len() + self.generic_struct_placeholders.len()
        );
        self.generic_struct_keys.insert(key, internal_name.clone());
        let substitutions = template
            .type_parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect::<HashMap<_, _>>();
        let mut specialized = template;
        specialized.name = internal_name.clone();
        specialized.type_parameters.clear();
        specialized.public = false;
        specialized.module_path.clear();
        for field in &mut specialized.fields {
            substitute_type_parameters(&mut field.ty, &substitutions);
            self.resolve_generic_placeholders(&mut field.ty, &substitutions)?;
        }
        self.generic_structs.push(specialized);
        Ok(internal_name)
    }

    fn specialize_generic_enum(
        &mut self,
        template_name: &str,
        arguments: &[TypeName],
        span: Span,
    ) -> Result<String, Diagnostic> {
        let template = self
            .generic_enum_templates
            .get(template_name)
            .cloned()
            .ok_or_else(|| self.error("unknown generic enum"))?;
        if template.type_parameters.len() != arguments.len() {
            return Err(Diagnostic {
                code: "R0244",
                message: format!(
                    "generic enum `{}` expects {} type argument(s), found {}",
                    template.name,
                    template.type_parameters.len(),
                    arguments.len()
                ),
                span,
                help: None,
            });
        }
        let key = format!(
            "{}<{}>",
            template_name,
            arguments.iter().map(type_key).collect::<Vec<_>>().join(",")
        );
        if arguments
            .iter()
            .any(|argument| self.contains_unresolved_generic_type(argument))
        {
            if let Some(name) = self.generic_enum_placeholder_keys.get(&key) {
                return Ok(name.clone());
            }
            let name = format!("$RynEnumParam#{}", self.generic_enum_placeholders.len());
            self.generic_enum_placeholders
                .insert(name.clone(), (template_name.to_owned(), arguments.to_vec()));
            self.generic_enum_placeholder_keys.insert(key, name.clone());
            return Ok(name);
        }
        if let Some(name) = self.generic_enum_keys.get(&key) {
            return Ok(name.clone());
        }
        let internal_name = format!(
            "$RynEnum#{template_name}#{}",
            self.generic_enum_keys.len() + self.generic_enum_placeholders.len()
        );
        self.generic_enum_keys.insert(key, internal_name.clone());
        let substitutions = template
            .type_parameters
            .iter()
            .cloned()
            .zip(arguments.iter().cloned())
            .collect::<HashMap<_, _>>();
        let mut specialized = template;
        specialized.name = internal_name.clone();
        specialized.type_parameters.clear();
        specialized.public = false;
        specialized.module_path.clear();
        for variant in &mut specialized.variants {
            for field in &mut variant.fields {
                substitute_type_parameters(field, &substitutions);
                self.resolve_generic_placeholders(field, &substitutions)?;
            }
        }
        self.generic_enums.push(specialized);
        Ok(internal_name)
    }

    fn contains_unresolved_generic_type(&self, ty: &TypeName) -> bool {
        match ty {
            TypeName::Parameter(_, _) => true,
            TypeName::Named(name, _) => {
                self.generic_struct_placeholders.contains_key(name)
                    || self.generic_enum_placeholders.contains_key(name)
            }
            TypeName::Vec(element, _)
            | TypeName::Array(element, _, _)
            | TypeName::Slice(element, _) => self.contains_unresolved_generic_type(element),
            TypeName::Map(key, value, _) => {
                self.contains_unresolved_generic_type(key)
                    || self.contains_unresolved_generic_type(value)
            }
            _ => false,
        }
    }

    fn resolve_generic_placeholders(
        &mut self,
        ty: &mut TypeName,
        substitutions: &HashMap<String, TypeName>,
    ) -> Result<(), Diagnostic> {
        if let TypeName::Named(name, span) = ty {
            let span = *span;
            let placeholder = self
                .generic_struct_placeholders
                .get(name)
                .map(|(template, arguments)| (true, template.clone(), arguments.clone()))
                .or_else(|| {
                    self.generic_enum_placeholders
                        .get(name)
                        .map(|(template, arguments)| (false, template.clone(), arguments.clone()))
                });
            if let Some((is_struct, template_name, mut arguments)) = placeholder {
                for argument in &mut arguments {
                    substitute_type_parameters(argument, substitutions);
                    self.resolve_generic_placeholders(argument, substitutions)?;
                }
                let specialized = if is_struct {
                    self.specialize_generic_struct(&template_name, &arguments, span)?
                } else {
                    self.specialize_generic_enum(&template_name, &arguments, span)?
                };
                *ty = TypeName::Named(specialized, span);
                return Ok(());
            }
        }
        match ty {
            TypeName::Vec(element, _)
            | TypeName::Array(element, _, _)
            | TypeName::Slice(element, _) => {
                self.resolve_generic_placeholders(element, substitutions)?;
            }
            TypeName::Map(key, value, _) => {
                self.resolve_generic_placeholders(key, substitutions)?;
                self.resolve_generic_placeholders(value, substitutions)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn register_tuple_type(&mut self, fields: Vec<TypeName>, span: Span) -> TypeName {
        let key = fields.iter().map(type_key).collect::<Vec<_>>().join(",");
        let name = if let Some(name) = self.tuple_type_names.get(&key) {
            name.clone()
        } else {
            let name = format!("$RynTuple#{key}");
            self.tuple_type_names.insert(key, name.clone());
            self.tuple_type_structs.push(StructDef {
                name: name.clone(),
                type_parameters: Vec::new(),
                fields: fields
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| StructField {
                        name: format!("_{index}"),
                        ty,
                        span,
                    })
                    .collect(),
                public: false,
                repr_c: false,
                drop_function: None,
                module_path: String::new(),
                span,
            });
            name
        };
        TypeName::Named(name, span)
    }

    fn expect_type_greater(&mut self, message: &str) -> Result<Token, Diagnostic> {
        if matches!(self.peek().kind, TokenKind::Greater) {
            return Ok(self.next());
        }
        if matches!(self.peek().kind, TokenKind::ShiftRight) {
            let combined = self.next();
            let first = Token {
                kind: TokenKind::Greater,
                span: Span {
                    start: combined.span.start,
                    end: combined.span.start + 1,
                },
            };
            self.tokens.insert(
                self.at,
                Token {
                    kind: TokenKind::Greater,
                    span: Span {
                        start: combined.span.start + 1,
                        end: combined.span.end,
                    },
                },
            );
            return Ok(first);
        }
        Err(self.error(message))
    }

    fn starts_statement(&self, allow_call: bool) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Mut
                | TokenKind::Echo
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Return
                | TokenKind::Star
        ) || self.field_assignment_start()
            || self.short_declaration_start()
            || self.index_assignment_start()
            || (matches!(self.peek().kind, TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.at + 1).map(|t| &t.kind),
                    Some(
                        TokenKind::Define
                            | TokenKind::Equal
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
                    Some(TokenKind::LParen | TokenKind::Dot)
                ))
    }

    fn starts_expression(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::Character(_)
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
                Some(TokenKind::Ident(_) | TokenKind::Integer(_))
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
                            Some(TokenKind::Dot) => false,
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
            TokenKind::Mut => self.declaration_statement(),
            TokenKind::Star => self.dereference_assignment(),
            TokenKind::Ident(_) if self.short_declaration_start() => self.declaration_statement(),
            TokenKind::Ident(_) if self.index_assignment_start() => self.index_assignment(),
            TokenKind::Echo => self.echo_statement(),
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
                    Some(TokenKind::LParen | TokenKind::Dot)
                ) =>
            {
                self.call_statement()
            }
            _ => Err(self.error(
                "expected a declaration, assignment, function call, `break`, `continue`, `return`, or `echo` statement",
            )),
        }
    }

    fn index_assignment(&mut self) -> Result<Statement, Diagnostic> {
        let (name, start) = self.ident("expected array variable")?;
        self.expect(
            |kind| matches!(kind, TokenKind::LBracket),
            "expected `[` after array variable",
        )?;
        let index = self.expression(0)?;
        self.expect(
            |kind| matches!(kind, TokenKind::RBracket),
            "expected `]` after array index",
        )?;
        self.expect(
            |kind| matches!(kind, TokenKind::Equal),
            "expected `=` after array index",
        )?;
        let value = self.expression(0)?;
        let span = Span {
            start: start.start,
            end: value.span().end,
        };
        Ok(Statement::IndexAssign {
            name,
            index,
            value,
            span,
        })
    }

    fn dereference_assignment(&mut self) -> Result<Statement, Diagnostic> {
        let target = self.expression(10)?;
        let Expression::Dereference(pointer, target_span) = target else {
            return Err(self.error("expected a dereference assignment target"));
        };
        self.expect(
            |kind| matches!(kind, TokenKind::Equal),
            "expected `=` after dereference assignment target",
        )?;
        let value = self.expression(0)?;
        let span = Span {
            start: target_span.start,
            end: value.span().end,
        };
        Ok(Statement::DereferenceAssign {
            pointer: *pointer,
            value,
            span,
        })
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
        let expression = self.expression(0)?;
        self.expression_statement(expression)
    }

    fn expression_statement(&self, expression: Expression) -> Result<Statement, Diagnostic> {
        match expression {
            Expression::Call {
                name,
                type_arguments,
                arguments,
                span,
            } => Ok(Statement::Call {
                name,
                type_arguments,
                arguments,
                span,
            }),
            Expression::MethodCall {
                value,
                name,
                arguments,
                span,
            } => Ok(Statement::MethodCall {
                value,
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
        let header: Result<(String, Span, ForHeader), Diagnostic> = (|| {
            let (name, name_span) = self.ident("expected loop variable after `for`")?;
            self.expect(
                |kind| matches!(kind, TokenKind::In),
                "expected `in` after the `for` loop variable",
            )?;
            let collection_or_start = self.expression(0)?;
            if matches!(self.peek().kind, TokenKind::DotDot) {
                self.next();
                let range_end = self.expression(0)?;
                Ok((
                    name,
                    name_span,
                    ForHeader::Range(collection_or_start, range_end),
                ))
            } else {
                Ok((name, name_span, ForHeader::Each(collection_or_start)))
            }
        })();
        // A valid expression parser may stop before an unexpected token. Make
        // the required body brace part of header validation so recovery can
        // still find and parse the loop body (for example `for i in 0 3 {}`).
        let header = header.and_then(|header| {
            if matches!(self.peek().kind, TokenKind::LBrace) {
                Ok(header)
            } else {
                Err(self.error("expected `{` to start block"))
            }
        });
        let (name, name_span, header, body) = match header {
            Ok((name, name_span, header)) => {
                let body = self.block_statements()?;
                (name, name_span, header, body)
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
                    ForHeader::Range(placeholder, Expression::Integer(0, error_span)),
                    body,
                )
            }
            Err(diagnostic) => return Err(diagnostic),
        };
        let end = self.tokens[self.at - 1].span.end;
        Ok(match header {
            ForHeader::Range(range_start, range_end) => Statement::For {
                name,
                name_span,
                start: range_start,
                end: range_end,
                body,
                span: Span { start, end },
            },
            ForHeader::Each(collection) => Statement::ForEach {
                name,
                name_span,
                collection,
                body,
                span: Span { start, end },
            },
        })
    }

    fn short_declaration_start(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_)) {
            return false;
        }
        if matches!(
            self.tokens.get(self.at + 1).map(|token| &token.kind),
            Some(TokenKind::Define)
        ) {
            return true;
        }
        if !matches!(
            self.tokens.get(self.at + 1).map(|token| &token.kind),
            Some(TokenKind::Colon)
        ) {
            return false;
        }
        let mut square_depth = 0usize;
        let mut generic_depth = 0usize;
        let mut paren_depth = 0usize;
        for token in self.tokens.iter().skip(self.at + 2) {
            match token.kind {
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen if paren_depth > 0 => paren_depth -= 1,
                TokenKind::LBracket => square_depth += 1,
                TokenKind::RBracket if square_depth > 0 => square_depth -= 1,
                TokenKind::Less => generic_depth += 1,
                TokenKind::Greater if generic_depth > 0 => generic_depth -= 1,
                TokenKind::ShiftRight if generic_depth >= 2 => generic_depth -= 2,
                TokenKind::Equal if square_depth == 0 && generic_depth == 0 && paren_depth == 0 => {
                    return true;
                }
                TokenKind::Eof | TokenKind::RBrace | TokenKind::Define => {
                    return false;
                }
                TokenKind::Comma if square_depth == 0 && generic_depth == 0 && paren_depth == 0 => {
                    return false;
                }
                _ => {}
            }
        }
        false
    }

    fn index_assignment_start(&self) -> bool {
        if !matches!(self.peek().kind, TokenKind::Ident(_))
            || !matches!(
                self.tokens.get(self.at + 1).map(|token| &token.kind),
                Some(TokenKind::LBracket)
            )
        {
            return false;
        }
        let mut depth = 0usize;
        for (index, token) in self.tokens.iter().enumerate().skip(self.at + 1) {
            match token.kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return self
                            .tokens
                            .get(index + 1)
                            .is_some_and(|token| matches!(token.kind, TokenKind::Equal));
                    }
                }
                TokenKind::Eof | TokenKind::RBrace => return false,
                _ => {}
            }
        }
        false
    }

    fn declaration_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.peek().span.start;
        let mutable = if matches!(self.peek().kind, TokenKind::Mut) {
            self.next();
            true
        } else {
            false
        };
        let (name, _) = self.ident("expected variable name after `mut`")?;
        let annotation = if matches!(self.peek().kind, TokenKind::Colon) {
            self.next();
            Some(self.type_name()?)
        } else {
            None
        };
        self.expect(
            |kind| {
                if annotation.is_some() {
                    matches!(kind, TokenKind::Equal)
                } else {
                    matches!(kind, TokenKind::Define)
                }
            },
            if annotation.is_some() {
                "expected `=` after the explicit type"
            } else {
                "expected `:=` for an inferred declaration"
            },
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
            let (field, field_span) = self.field_name_after_dot()?;
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

    fn field_name_after_dot(&mut self) -> Result<(String, Span), Diagnostic> {
        match self.next() {
            Token {
                kind: TokenKind::Ident(name),
                span,
            } => Ok((name, span)),
            Token {
                kind: TokenKind::Integer(index),
                span,
            } => Ok((format!("_{index}"), span)),
            token => Err(Diagnostic {
                code: "R0010",
                message: "expected field name or tuple index after `.`".into(),
                span: token.span,
                help: None,
            }),
        }
    }

    fn echo_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.next().span.start;
        let grouped_template = matches!(self.peek().kind, TokenKind::LParen)
            && self.tokens.get(self.at + 1).is_some_and(|token| {
                matches!(&token.kind, TokenKind::String(value) if value.contains('{') || value.contains('}'))
            });
        if grouped_template {
            self.next();
        }
        if let TokenKind::String(value) = &self.peek().kind
            && (value.contains('{') || value.contains('}'))
        {
            let token = self.next();
            let TokenKind::String(value) = token.kind else {
                return Err(Diagnostic {
                    code: "R0010",
                    message: "expected string literal after `echo`".into(),
                    span: token.span,
                    help: None,
                });
            };
            let Some(parts) = self.print_template(&value, token.span)? else {
                return Err(Diagnostic {
                    code: "R0010",
                    message: "expected echo interpolation".into(),
                    span: token.span,
                    help: None,
                });
            };
            let end = if grouped_template {
                self.expect(
                    |kind| matches!(kind, TokenKind::RParen),
                    "expected `)` after echo template",
                )?
                .span
                .end
            } else {
                token.span.end
            };
            return Ok(Statement::PrintTemplate(parts, Span { start, end }));
        }
        let value = self.expression(0)?;
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
                            message: "unterminated echo interpolation".into(),
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
                            message: "echo interpolation must contain a variable name".into(),
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
                        message: "unmatched `}` in echo interpolation".into(),
                        span: mapped_span(&offsets, raw_end, at, at + 1),
                        help: Some("write `}}` to echo a literal `}`".into()),
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
                kind: TokenKind::LBracket,
                span,
            } => {
                let mut elements = Vec::new();
                if !matches!(self.peek().kind, TokenKind::RBracket) {
                    loop {
                        elements.push(self.expression(0)?);
                        if !matches!(self.peek().kind, TokenKind::Comma) {
                            break;
                        }
                        self.next();
                        if matches!(self.peek().kind, TokenKind::RBracket) {
                            break;
                        }
                    }
                }
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RBracket),
                        "expected `]` after array elements",
                    )?
                    .span
                    .end;
                Expression::ArrayLiteral(
                    elements,
                    Span {
                        start: span.start,
                        end,
                    },
                )
            }
            Token {
                kind: TokenKind::Character(value),
                span,
            } => Expression::Character(value, span),
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
            } if matches!(name.as_str(), "Map" | "HashMap" | "Set")
                && matches!(self.peek().kind, TokenKind::Less) =>
            {
                self.next();
                let key = self.type_name()?;
                let value = if name == "Set" {
                    TypeName::Bool
                } else {
                    self.expect(
                        |kind| matches!(kind, TokenKind::Comma),
                        "expected `,` between Map key and value types",
                    )?;
                    self.type_name()?
                };
                self.expect_type_greater("expected `>` after Map key and value types")?;
                self.ensure_map_option_specialization(&value, span);
                self.expect(
                    |kind| matches!(kind, TokenKind::LParen),
                    "expected `()` after `Map<K, V>`",
                )?;
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RParen),
                        "expected empty `()` after `Map<K, V>`",
                    )?
                    .span
                    .end;
                Expression::MapConstructor {
                    key,
                    value,
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } if matches!(name.as_str(), "sizeof" | "alignof")
                && matches!(self.peek().kind, TokenKind::LParen) =>
            {
                self.next();
                let ty = self.type_name()?;
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RParen),
                        "expected `)` after layout type",
                    )?
                    .span
                    .end;
                Expression::LayoutOf {
                    ty,
                    alignment: name == "alignof",
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } if name == "Vec" && matches!(self.peek().kind, TokenKind::Less) => {
                self.next();
                let element = self.type_name()?;
                self.expect(
                    |kind| matches!(kind, TokenKind::Greater),
                    "expected `>` after the `Vec` element type",
                )?;
                if matches!(self.peek().kind, TokenKind::LParen) {
                    self.next();
                    let end = self
                        .expect(
                            |kind| matches!(kind, TokenKind::RParen),
                            "expected `()`, e.g. `Vec<i32>()`",
                        )?
                        .span
                        .end;
                    Expression::VecConstructor {
                        element,
                        span: Span {
                            start: span.start,
                            end,
                        },
                    }
                } else {
                    return Err(
                        self.error("expected `()` after `Vec<T>` to construct an empty vector")
                    );
                }
            }
            Token {
                kind: TokenKind::Ident(name),
                span,
            } if name != "Vec" && matches!(self.peek().kind, TokenKind::ColonColon) => {
                let mut path = vec![name];
                let mut type_arguments = Vec::new();
                while matches!(self.peek().kind, TokenKind::ColonColon) {
                    self.next();
                    if matches!(self.peek().kind, TokenKind::Less) {
                        self.next();
                        loop {
                            type_arguments.push(self.type_name()?);
                            if matches!(self.peek().kind, TokenKind::Comma) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                        self.expect_type_greater("expected `>` after generic call arguments")?;
                        break;
                    }
                    path.push(self.ident("expected path segment after `::`")?.0);
                }
                if !type_arguments.is_empty()
                    && matches!(self.peek().kind, TokenKind::ColonColon)
                    && self.generic_enum_templates.contains_key(&path.join("::"))
                {
                    let template_name = path.join("::");
                    let specialized =
                        self.specialize_generic_enum(&template_name, &type_arguments, span)?;
                    self.next();
                    let variant = self.ident("expected variant after generic enum type")?.0;
                    path = vec![specialized, variant];
                    type_arguments.clear();
                }
                if !type_arguments.is_empty()
                    && matches!(self.peek().kind, TokenKind::LBrace)
                    && self.generic_struct_templates.contains_key(&path.join("::"))
                {
                    let template_name = path.join("::");
                    path = vec![self.specialize_generic_struct(
                        &template_name,
                        &type_arguments,
                        span,
                    )?];
                }
                if matches!(self.peek().kind, TokenKind::LParen) {
                    self.next();
                    let mut arguments = Vec::new();
                    if !matches!(self.peek().kind, TokenKind::RParen) {
                        loop {
                            arguments.push(self.expression(0)?);
                            if matches!(self.peek().kind, TokenKind::Comma) {
                                self.next();
                            } else {
                                break;
                            }
                        }
                    }
                    let end = self
                        .expect(
                            |kind| matches!(kind, TokenKind::RParen),
                            "expected `)` after enum variant arguments",
                        )?
                        .span
                        .end;
                    Expression::Call {
                        name: path.join("::"),
                        type_arguments,
                        arguments,
                        span: Span {
                            start: span.start,
                            end,
                        },
                    }
                } else if matches!(self.peek().kind, TokenKind::LBrace)
                    && matches!(
                        self.tokens.get(self.at + 1).map(|token| &token.kind),
                        Some(TokenKind::Ident(_))
                    )
                    && matches!(
                        self.tokens.get(self.at + 2).map(|token| &token.kind),
                        Some(TokenKind::Colon)
                    )
                {
                    self.next();
                    let mut fields = Vec::new();
                    while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
                        let (field, field_span) =
                            self.ident("expected field name in structure literal")?;
                        self.expect(
                            |kind| matches!(kind, TokenKind::Colon),
                            "expected `:` after structure literal field",
                        )?;
                        let value = self.expression(0)?;
                        fields.push((field, value, field_span));
                        if matches!(self.peek().kind, TokenKind::Comma) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                    let end = self
                        .expect(
                            |kind| matches!(kind, TokenKind::RBrace),
                            "expected `}` after structure literal",
                        )?
                        .span
                        .end;
                    Expression::StructLiteral {
                        name: path.join("::"),
                        fields,
                        span: Span {
                            start: span.start,
                            end,
                        },
                    }
                } else if path.len() >= 2 {
                    let variant = path.pop().unwrap();
                    let enum_name = path.join("::");
                    let end = self.tokens[self.at - 1].span.end;
                    Expression::EnumConstruct {
                        enum_name,
                        variant,
                        arguments: Vec::new(),
                        span: Span {
                            start: span.start,
                            end,
                        },
                    }
                } else {
                    return Err(self.error("expected a function call or enum variant after `::`"));
                }
            }
            Token {
                kind: TokenKind::Choose,
                span,
            } => {
                let value = self.expression(0)?;
                self.expect(
                    |kind| matches!(kind, TokenKind::LBrace),
                    "expected `{` after `choose` value",
                )?;
                let mut arms = Vec::new();
                while !matches!(self.peek().kind, TokenKind::RBrace | TokenKind::Eof) {
                    let arm_start = self.at;
                    let arm = (|| -> Result<ChooseArm, Diagnostic> {
                        let (enum_name, variant, bindings) = if matches!(
                            self.peek().kind,
                            TokenKind::Ident(ref name) if name == "_"
                        ) {
                            self.next();
                            (None, None, Vec::new())
                        } else {
                            let (first, _) = self.ident("expected enum name in pattern")?;
                            let mut path = vec![first];
                            let mut type_arguments = Vec::new();
                            while matches!(self.peek().kind, TokenKind::ColonColon) {
                                self.next();
                                if matches!(self.peek().kind, TokenKind::Less) {
                                    self.next();
                                    loop {
                                        type_arguments.push(self.type_name()?);
                                        if matches!(self.peek().kind, TokenKind::Comma) {
                                            self.next();
                                        } else {
                                            break;
                                        }
                                    }
                                    self.expect_type_greater(
                                        "expected `>` after generic enum pattern arguments",
                                    )?;
                                    break;
                                }
                                path.push(
                                    self.ident("expected pattern path segment after `::`")?.0,
                                );
                            }
                            if !type_arguments.is_empty()
                                && matches!(self.peek().kind, TokenKind::ColonColon)
                                && self.generic_enum_templates.contains_key(&path.join("::"))
                            {
                                let template_name = path.join("::");
                                let specialized = self.specialize_generic_enum(
                                    &template_name,
                                    &type_arguments,
                                    self.tokens[arm_start].span,
                                )?;
                                self.next();
                                let variant =
                                    self.ident("expected variant after generic enum type")?.0;
                                path = vec![specialized, variant];
                            }
                            if path.len() < 2 {
                                return Err(
                                    self.error("expected `::Variant` after enum name in pattern")
                                );
                            }
                            let variant = path.pop().unwrap();
                            let enum_name = path.join("::");
                            let mut bindings = Vec::new();
                            if matches!(self.peek().kind, TokenKind::LParen) {
                                self.next();
                                if !matches!(self.peek().kind, TokenKind::RParen) {
                                    loop {
                                        bindings.push(self.ident("expected binding name")?.0);
                                        if matches!(self.peek().kind, TokenKind::Comma) {
                                            self.next();
                                        } else {
                                            break;
                                        }
                                    }
                                }
                                self.expect(
                                    |kind| matches!(kind, TokenKind::RParen),
                                    "expected `)` after bindings",
                                )?;
                            }
                            (Some(enum_name), Some(variant), bindings)
                        };
                        self.expect(
                            |kind| matches!(kind, TokenKind::FatArrow),
                            "expected `=>` after choose pattern",
                        )?;
                        let body = self.expression(0)?;
                        Ok(ChooseArm {
                            enum_name,
                            variant,
                            bindings,
                            body,
                            span: Span {
                                start: self.tokens[arm_start].span.start,
                                end: self.tokens[self.at - 1].span.end,
                            },
                        })
                    })();
                    match arm {
                        Ok(arm) => arms.push(arm),
                        Err(diagnostic) if self.recovering => {
                            self.recovery_diagnostics.push(diagnostic);
                            if self.at == arm_start {
                                self.next();
                            }
                            while !matches!(
                                self.peek().kind,
                                TokenKind::Comma | TokenKind::RBrace | TokenKind::Eof
                            ) {
                                self.next();
                            }
                        }
                        Err(diagnostic) => return Err(diagnostic),
                    }
                    if matches!(self.peek().kind, TokenKind::Comma) {
                        self.next();
                    } else if !matches!(self.peek().kind, TokenKind::RBrace) {
                        break;
                    }
                }
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RBrace),
                        "expected `}` after choose arms",
                    )?
                    .span
                    .end;
                Expression::Choose {
                    value: Box::new(value),
                    arms,
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
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
                if name == "read_file_result" {
                    self.ensure_result_specialization(
                        TypeName::OwnedString,
                        TypeName::I32,
                        Span {
                            start: span.start,
                            end,
                        },
                    );
                }
                Expression::Call {
                    name,
                    type_arguments: Vec::new(),
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
                kind: TokenKind::BitAnd,
                span,
            } => {
                let raw = matches!(&self.peek().kind, TokenKind::Ident(name) if name == "raw");
                if raw {
                    self.next();
                }
                let mutable = self.consume_if(|kind| matches!(kind, TokenKind::Mut));
                let value = self.expression(10)?;
                let end = value.span().end;
                Expression::AddressOf {
                    mutable,
                    raw,
                    value: Box::new(value),
                    span: Span {
                        start: span.start,
                        end,
                    },
                }
            }
            Token {
                kind: TokenKind::Star,
                span,
            } => {
                let value = self.expression(10)?;
                let end = value.span().end;
                Expression::Dereference(
                    Box::new(value),
                    Span {
                        start: span.start,
                        end,
                    },
                )
            }
            Token {
                kind: TokenKind::LParen,
                span: open_span,
            } => {
                if matches!(self.peek().kind, TokenKind::RParen) {
                    let end = self.next().span.end;
                    return Err(Diagnostic {
                        code: "R0012",
                        message: "empty tuple values are not supported".into(),
                        span: Span {
                            start: open_span.start,
                            end,
                        },
                        help: Some("use a tuple with at least one element".into()),
                    });
                }
                let first = self.expression(0)?;
                if matches!(self.peek().kind, TokenKind::Comma) {
                    let mut values = vec![first];
                    while matches!(self.peek().kind, TokenKind::Comma) {
                        self.next();
                        if matches!(self.peek().kind, TokenKind::RParen) {
                            break;
                        }
                        values.push(self.expression(0)?);
                    }
                    let end = self
                        .expect(
                            |kind| matches!(kind, TokenKind::RParen),
                            "expected `)` after tuple elements",
                        )?
                        .span
                        .end;
                    Expression::Tuple(
                        values,
                        Span {
                            start: open_span.start,
                            end,
                        },
                    )
                } else {
                    self.expect(
                        |kind| matches!(kind, TokenKind::RParen),
                        "expected `)` after expression",
                    )?;
                    first
                }
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
            if matches!(self.peek().kind, TokenKind::LBracket) {
                if postfix_depth >= MAX_EXPRESSION_DEPTH {
                    return Err(self.nesting_error("indexing"));
                }
                postfix_depth += 1;
                self.next();
                let index = self.expression(0)?;
                let end = self
                    .expect(
                        |kind| matches!(kind, TokenKind::RBracket),
                        "expected `]` after index",
                    )?
                    .span
                    .end;
                let start = left.span().start;
                left = Expression::Index {
                    value: Box::new(left),
                    index: Box::new(index),
                    span: Span { start, end },
                };
                continue;
            }
            if matches!(self.peek().kind, TokenKind::Question) {
                if postfix_depth >= MAX_EXPRESSION_DEPTH {
                    return Err(self.nesting_error("error propagation"));
                }
                postfix_depth += 1;
                let end = self.next().span.end;
                let span = Span {
                    start: left.span().start,
                    end,
                };
                left = Expression::Propagate(Box::new(left), span);
                continue;
            }
            if matches!(self.peek().kind, TokenKind::Dot) {
                if postfix_depth >= MAX_EXPRESSION_DEPTH {
                    return Err(self.nesting_error("field access"));
                }
                postfix_depth += 1;
                self.next();
                let (name, field_span) = self.field_name_after_dot()?;
                let span = Span {
                    start: left.span().start,
                    end: field_span.end,
                };
                left = if matches!(self.peek().kind, TokenKind::LParen) {
                    self.next();
                    let mut arguments = Vec::new();
                    while !matches!(self.peek().kind, TokenKind::RParen | TokenKind::Eof) {
                        arguments.push(self.expression(0)?);
                        if !matches!(self.peek().kind, TokenKind::Comma) {
                            break;
                        }
                        self.next();
                    }
                    let end = self
                        .expect(
                            |kind| matches!(kind, TokenKind::RParen),
                            "expected `)` after method arguments",
                        )?
                        .span
                        .end;
                    Expression::MethodCall {
                        value: Box::new(left),
                        name,
                        arguments,
                        span: Span {
                            start: span.start,
                            end,
                        },
                    }
                } else {
                    Expression::Field {
                        value: Box::new(left),
                        name,
                        name_span: field_span,
                        span,
                    }
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
            if matches!(self.peek().kind, TokenKind::Star)
                && matches!(
                    self.tokens.get(self.at + 1).map(|token| &token.kind),
                    Some(TokenKind::Ident(_))
                )
                && matches!(
                    self.tokens.get(self.at + 2).map(|token| &token.kind),
                    Some(TokenKind::Equal)
                )
            {
                break;
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
        let then_value = self.expression_block("expected `{` after when-expression condition")?;
        self.expect(
            |kind| matches!(kind, TokenKind::Else),
            "when expression requires an `else` branch",
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
            "expected `}` after when-expression value",
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
    fn consume_if(&mut self, test: impl FnOnce(&TokenKind) -> bool) -> bool {
        if test(&self.peek().kind) {
            self.next();
            true
        } else {
            false
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
        TypeName::Map(key, value, _) => {
            format!("Map<{},{}>", type_key(key), type_key(value))
        }
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

fn substitute_type_parameters(ty: &mut TypeName, substitutions: &HashMap<String, TypeName>) {
    match ty {
        TypeName::Parameter(name, _) => {
            if let Some(replacement) = substitutions.get(name) {
                *ty = replacement.clone();
            }
        }
        TypeName::Vec(element, _)
        | TypeName::Array(element, _, _)
        | TypeName::Slice(element, _) => substitute_type_parameters(element, substitutions),
        TypeName::Map(key, value, _) => {
            substitute_type_parameters(key, substitutions);
            substitute_type_parameters(value, substitutions);
        }
        _ => {}
    }
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
            | "str"
            | "String"
            | "char"
            | "bool"
            | "Vec"
            | "Map"
            | "HashMap"
            | "Set"
            | "Option"
            | "Result"
    )
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
        let program = parse("fun main() { answer := 20 + 2 * 11 echo(answer) }")
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
        let program = parse("fun main() { echo(1 | 2 ^ 3 & 4 == 5 && true || false) }")
            .expect("bitwise expression parses");
        let Statement::Print(expression, _) = &program.functions[0].body[0] else {
            panic!("expected echo statement");
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
        let source = "fun first() { mut := 1 }\nfun second( { }\nfun main() {}";
        let diagnostics =
            parse_recovering(source).expect_err("both malformed declarations should be reported");

        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].message.contains("expected variable name"));
        assert!(diagnostics[1].message.contains("expected parameter name"));
        assert!(parse("fun main() {}").is_ok());
    }

    #[test]
    fn parser_recovery_reports_independent_function_parameter_errors() {
        let source = "fun broken(a i32, : bool, c: ) -> i32 { echo(1 + ) 1 } fun main() {}";
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
        assert!(parse("fun broken(value: i32,) { }").is_err());

        let missing_comma = parse_recovering("fun broken(first: i32 second: i32) { }")
            .expect_err("function parameters still require commas");
        assert_eq!(missing_comma.len(), 1);
        assert!(missing_comma[0].message.contains("expected `,`"));
    }

    #[test]
    fn parser_recovery_reports_independent_function_argument_errors() {
        let source = "fun main() { echo(combine(1 + , 2 + , 3)) mut := 4 } fun combine(a: i32, b: i32, c: i32) {}";
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
        assert!(parse("fun main() { combine(1,) }").is_err());

        let malformed_if = parse_recovering("fun main() { combine(when true { 1 }, 2) }")
            .expect_err("a missing branch should be diagnosed without losing the next argument");
        assert_eq!(malformed_if.len(), 1, "{malformed_if:?}");
        assert!(malformed_if[0].message.contains("requires an `else`"));

        let missing_comma = parse_recovering("fun main() { combine(1 2) }")
            .expect_err("a missing argument separator should be diagnosed");
        assert_eq!(missing_comma.len(), 1, "{missing_comma:?}");
        assert!(missing_comma[0].message.contains("expected `,`"));
        assert!(parse("fun main() { combine(1 2) }").is_err());
    }

    #[test]
    fn parser_recovery_reports_independent_structure_literal_value_errors() {
        let source = "struct Pair { a: i32, b: i32, c: i32 } fun main() { value := Pair { a: 1 + , b: 2 + , c: } mut := 3 }";
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
            "struct Pair { a: i32, b: i32 } fun main() { pair := Pair { a: when true { 1 }, b: 2 } }",
        )
        .expect_err("a missing branch should not hide later structure fields");
        assert_eq!(malformed_if.len(), 1, "{malformed_if:?}");
        assert!(malformed_if[0].message.contains("requires an `else`"));

        let missing_comma = parse_recovering(
            "struct Pair { a: i32, b: i32 } fun main() { pair := Pair { a: 1 b: 2 } }",
        )
        .expect_err("a missing structure literal separator should be diagnosed");
        assert_eq!(missing_comma.len(), 1, "{missing_comma:?}");
        assert!(missing_comma[0].message.contains("expected `,`"));
        assert!(parse("struct Pair { a: i32 } fun main() { pair := Pair { a: 1 b: 2 } }").is_err());
    }

    #[test]
    fn parser_recovery_reports_independent_structure_field_syntax_errors() {
        let source =
            "struct Config { good: i32, broken: , next: str, : bool, last: f64 } fun main() {}";
        let diagnostics = parse_recovering(source)
            .expect_err("both malformed structure fields should be reported");

        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        assert!(diagnostics[0].message.contains("expected type name"));
        assert!(diagnostics[1].message.contains("expected field name"));
        assert!(diagnostics[0].span.start < diagnostics[1].span.start);
    }

    #[test]
    fn parser_recovery_reports_independent_errors_inside_nested_blocks() {
        let source = "fun main() {\n    mut := 1\n    when true {\n        echo(1 + )\n        mut := 2\n        echo(3)\n    }\n    mut := 4\n}\nfun other() { echo(5 + ) }";
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
        let source = "fun main() { when ) { mut := 1 echo(2 + ) } while { mut := 3 } }";
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
        let source = "fun main() { for index in 0.. { mut := 1 echo(2 + ) } echo(3) }";
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
            "fun main() { for in 0..3 { mut := 1 echo(2 + ) } }",
            "fun main() { for index 0..3 { mut := 1 echo(2 + ) } }",
            "fun main() { for index in ..3 { mut := 1 echo(2 + ) } }",
            "fun main() { for index in 0 3 { mut := 1 echo(2 + ) } }",
        ] {
            let diagnostics = parse_recovering(source)
                .expect_err("malformed range headers should still check the loop body");

            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("expected variable name after `mut`")),
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
            "fun main() { for index in 0.. when true { 1 + } else { 2 } { mut := 3 } }",
            "fun main() { while when true { 1 + } else { true } { mut := 4 } }",
            "fun main() { for index in 0.. when ) { 1 } else { 2 } { mut := 3 } }",
            "fun main() { while when ) { true } else { true } { mut := 4 } }",
            "struct Point { x: i32 } fun main() { for index in 0.. when Point { x: 1 + } == Point { x: 0 } { 1 } else { 2 } { mut := 3 } }",
            "struct Point { x: i32 } fun main() { while when Point { x: 1 + } == Point { x: 0 } { true } else { false } { mut := 4 } }",
        ] {
            let diagnostics = parse_recovering(source)
                .expect_err("malformed when expressions and loop bodies should be reported");

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
        let diagnostics = parse_recovering("@ fun main() { 💥 }")
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
            parse("fun main() { answer := when true { 1 } else when false { 2 } else { 3 } }")
                .expect("when expression parses");
        let Statement::Let {
            value: Expression::If {
                else_value, span, ..
            },
            ..
        } = &program.functions[0].body[0]
        else {
            panic!("inferred declaration should contain a when expression");
        };

        assert!(matches!(**else_value, Expression::If { .. }));
        assert_eq!(
            &"fun main() { answer := when true { 1 } else when false { 2 } else { 3 } }"
                [span.start..span.end],
            "when true { 1 } else when false { 2 } else { 3 }"
        );
    }

    #[test]
    fn requires_if_expression_branches_to_be_value_blocks_with_an_else() {
        let missing_else = parse("fun main() { answer := when true { 1 } }")
            .expect_err("when expression without else is incomplete");
        assert_eq!(
            missing_else.message,
            "when expression requires an `else` branch"
        );

        let statement_branch =
            parse("fun main() { answer := when true { value := 1 } else { 2 } }")
                .expect_err("when expression branch must produce a value");
        assert_eq!(
            statement_branch.message,
            "expected `}` after when-expression value"
        );
    }

    #[test]
    fn deeply_nested_if_expressions_return_a_diagnostic_without_panicking() {
        let mut expression = "0".to_string();
        for _ in 0..=super::MAX_EXPRESSION_DEPTH {
            expression = format!("when true {{ {expression} }} else {{ 0 }}");
        }
        let source = format!("fun main() {{ value := {expression} }}");

        let diagnostic = parse(&source).expect_err("excessive expression nesting is rejected");
        assert_eq!(diagnostic.code, "R0015");
        assert!(diagnostic.message.contains("maximum parser nesting depth"));
    }

    #[test]
    fn rejects_missing_closing_brace() {
        assert!(parse("fun main() { echo(\"Hi\")").is_err());
    }

    #[test]
    fn malformed_echo_interpolation_has_a_parser_diagnostic() {
        let source = "fun main() { echo(\"Hello, {name\") }";
        let diagnostic = parse(source).expect_err("unterminated interpolation is invalid");
        assert_eq!(diagnostic.code, "R0014");
        assert_eq!(diagnostic.message, "unterminated echo interpolation");
        assert_eq!(
            diagnostic.span.start,
            source.find("{name").expect("opening brace is present")
        );
    }

    #[test]
    fn deeply_nested_expressions_return_a_diagnostic_without_panicking() {
        let source = format!(
            "fun main() {{ echo({}true{}) }}",
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
            "fun main() {{ echo {}true{} }}",
            "(".repeat(nesting),
            ")".repeat(nesting),
        );

        parse(&source).expect("nesting at the supported limit must parse");
    }

    #[test]
    fn deeply_nested_blocks_return_a_diagnostic_without_panicking() {
        let source = format!(
            "fun main() {{ {}echo(true){} }}",
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
            "fun main() {{ when false {{}}{} else {{}} }}",
            " else when false {}".repeat(super::MAX_IF_DEPTH),
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
            "fun main() {{ when false {{}}{} else {{}} }}",
            " else when false {}".repeat(super::MAX_IF_DEPTH - 1),
        );

        parse(&source).expect("conditional nesting at the supported limit must parse");
    }

    #[test]
    fn block_nesting_at_the_limit_is_accepted() {
        let source = format!(
            "fun main() {{ {}echo(true){} }}",
            "while true {".repeat(super::MAX_BLOCK_DEPTH),
            "}".repeat(super::MAX_BLOCK_DEPTH),
        );

        parse(&source).expect("nesting at the supported limit must parse");
    }

    #[test]
    fn reports_a_missing_function_name_at_eof_without_panicking() {
        let diagnostic = parse("fun").expect_err("function name is missing");
        assert_eq!(diagnostic.code, "R0010");
        assert_eq!(diagnostic.message, "expected function name");
    }

    #[test]
    fn parses_canonical_bindings_conditions_and_echo_statements() {
        parse(
            "fun main() { name := \"Ryn\" age: i32 = 1 mut counter := 0 mut limit: i32 = 2 counter += 1 when counter > 0 { echo \"Hello {name}\" echo limit echo(counter) } }",
        )
        .expect("canonical syntax parses");
    }

    #[test]
    fn rejects_removed_keyword_and_output_spellings() {
        for source in [
            "fn main() {}",
            "fun main() { let value = 1 }",
            "fun main() { if true { echo 1 } }",
        ] {
            assert!(
                parse(source).is_err(),
                "removed syntax was accepted: {source}"
            );
        }
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
            "fun",
            "struct",
            "name",
            "(",
            ")",
            "{",
            "}",
            "mut",
            ":",
            "=",
            "->",
            "return",
            "when",
            "else",
            "while",
            "echo",
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
            parse("fun answer() -> i32 { return 42 }").expect("explicit return statement parses");

        assert!(matches!(
            program.functions[0].body[0],
            Statement::Return { .. }
        ));
    }

    #[test]
    fn field_access_chains_obey_the_expression_depth_limit() {
        let path = format!("value{}", ".field".repeat(super::MAX_EXPRESSION_DEPTH + 1));
        let error = parse(&format!("fun main() {{ echo({path}) }}"))
            .expect_err("excessive field access nesting is rejected");
        assert_eq!(error.code, "R0015");
        assert!(error.message.contains("field access"));

        let assignment = format!(
            "value{} = 1",
            ".field".repeat(super::MAX_EXPRESSION_DEPTH + 1)
        );
        let error = parse(&format!("fun main() {{ {assignment} }}"))
            .expect_err("excessive field assignment nesting is rejected");
        assert_eq!(error.code, "R0015");
    }
}
