use crate::source::{Diagnostic, Span};
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Fn,
    Mut,
    True,
    False,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    Echo,
    Struct,
    As,
    Enum,
    TypeAlias,
    Choose,
    Defer,
    Use,
    Namespace,
    Pub,
    Extend,
    Shape,
    Const,
    Ident(String),
    String(String),
    Character(char),
    Integer(u64),
    Float(f64),
    EqualEqual,
    Bang,
    Question,
    QuestionQuestion,
    DotDotEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    BitAnd,
    BitAndEqual,
    BitOr,
    BitOrEqual,
    ShiftLeft,
    ShiftLeftEqual,
    ShiftRight,
    ShiftRightEqual,
    Caret,
    CaretEqual,
    Tilde,
    AndAnd,
    OrOr,
    PipeGreater,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    ColonColon,
    Define,
    Dot,
    DotDot,
    Equal,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Arrow,
    FatArrow,
    Hash,
    Eof,
}
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub fn lex(text: &str) -> Result<Vec<Token>, Diagnostic> {
    let (tokens, mut diagnostics) = lex_all(text, false);
    if diagnostics.is_empty() {
        Ok(tokens)
    } else {
        Err(diagnostics.remove(0))
    }
}

/// Tokenizes a source file and gathers diagnostics from recoverable malformed
/// tokens. At most one diagnostic is emitted for a malformed string token.
/// An unterminated string or block comment ends recovery because the remaining
/// input belongs to that token.
pub fn lex_recovering(text: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let (tokens, diagnostics) = lex_all(text, true);
    if diagnostics.is_empty() {
        Ok(tokens)
    } else {
        Err(diagnostics)
    }
}

fn lex_all(text: &str, recovering: bool) -> (Vec<Token>, Vec<Diagnostic>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();
    'tokens: while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && !matches!(bytes[i], b'\n' | b'\r') {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            let start = i;
            match skip_block_comment(bytes, start) {
                Some(end) => i = end,
                None => {
                    diagnostics.push(Diagnostic {
                        code: "R0018",
                        message: "unterminated block comment".into(),
                        span: Span {
                            start,
                            end: bytes.len(),
                        },
                        help: Some("close the comment with `*/`".into()),
                    });
                    break 'tokens;
                }
            }
            continue;
        }
        let start = i;
        let kind = match bytes[i] {
            b'(' => {
                i += 1;
                TokenKind::LParen
            }
            b')' => {
                i += 1;
                TokenKind::RParen
            }
            b'[' => {
                i += 1;
                TokenKind::LBracket
            }
            b']' => {
                i += 1;
                TokenKind::RBracket
            }
            b'{' => {
                i += 1;
                TokenKind::LBrace
            }
            b'}' => {
                i += 1;
                TokenKind::RBrace
            }
            b',' => {
                i += 1;
                TokenKind::Comma
            }
            b';' => {
                i += 1;
                TokenKind::Semicolon
            }
            b':' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::Define
            }
            b':' if bytes.get(i + 1) == Some(&b':') => {
                i += 2;
                TokenKind::ColonColon
            }
            b':' => {
                i += 1;
                TokenKind::Colon
            }
            b'.' if bytes.get(i + 1) == Some(&b'.') && bytes.get(i + 2) == Some(&b'=') => {
                i += 3;
                TokenKind::DotDotEqual
            }
            b'.' if bytes.get(i + 1) == Some(&b'.') => {
                i += 2;
                TokenKind::DotDot
            }
            b'.' => {
                i += 1;
                TokenKind::Dot
            }
            b'=' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::EqualEqual
            }
            b'=' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::FatArrow
            }
            b'=' => {
                i += 1;
                TokenKind::Equal
            }
            b'!' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::BangEqual
            }
            b'!' => {
                i += 1;
                TokenKind::Bang
            }
            b'?' if bytes.get(i + 1) == Some(&b'?') => {
                i += 2;
                TokenKind::QuestionQuestion
            }
            b'?' => {
                i += 1;
                TokenKind::Question
            }
            b'<' if bytes.get(i + 2) == Some(&b'=') && bytes.get(i + 1) == Some(&b'<') => {
                i += 3;
                TokenKind::ShiftLeftEqual
            }
            b'<' if bytes.get(i + 1) == Some(&b'<') => {
                i += 2;
                TokenKind::ShiftLeft
            }
            b'<' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::LessEqual
            }
            b'<' => {
                i += 1;
                TokenKind::Less
            }
            b'>' if bytes.get(i + 2) == Some(&b'=') && bytes.get(i + 1) == Some(&b'>') => {
                i += 3;
                TokenKind::ShiftRightEqual
            }
            b'>' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::ShiftRight
            }
            b'>' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::GreaterEqual
            }
            b'>' => {
                i += 1;
                TokenKind::Greater
            }
            b'&' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::BitAndEqual
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                i += 2;
                TokenKind::AndAnd
            }
            b'&' => {
                i += 1;
                TokenKind::BitAnd
            }
            b'|' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::BitOrEqual
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => {
                i += 2;
                TokenKind::OrOr
            }
            b'|' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::PipeGreater
            }
            b'|' => {
                i += 1;
                TokenKind::BitOr
            }
            b'^' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::CaretEqual
            }
            b'^' => {
                i += 1;
                TokenKind::Caret
            }
            b'~' => {
                i += 1;
                TokenKind::Tilde
            }
            b'#' => {
                i += 1;
                TokenKind::Hash
            }
            b'+' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::PlusEqual
            }
            b'+' => {
                i += 1;
                TokenKind::Plus
            }
            b'-' if bytes.get(i + 1) == Some(&b'>') => {
                i += 2;
                TokenKind::Arrow
            }
            b'-' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::MinusEqual
            }
            b'-' => {
                i += 1;
                TokenKind::Minus
            }
            b'*' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::StarEqual
            }
            b'*' => {
                i += 1;
                TokenKind::Star
            }
            b'/' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::SlashEqual
            }
            b'/' => {
                i += 1;
                TokenKind::Slash
            }
            b'%' if bytes.get(i + 1) == Some(&b'=') => {
                i += 2;
                TokenKind::PercentEqual
            }
            b'%' => {
                i += 1;
                TokenKind::Percent
            }
            b'\'' => match character_literal(text, i) {
                Ok((character, end)) => {
                    i = end;
                    TokenKind::Character(character)
                }
                Err(diagnostic) => {
                    i = diagnostic.span.end;
                    diagnostics.push(diagnostic);
                    continue 'tokens;
                }
            },
            b'"' => {
                i += 1;
                let mut value = String::new();
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                        if i == bytes.len() {
                            break;
                        }
                        if bytes[i] == b'u' {
                            let escape_start = i - 1;
                            match decode_unicode_escape(text, i) {
                                Ok((character, end)) => {
                                    value.push(character);
                                    i = end;
                                    continue;
                                }
                                Err(end) => {
                                    diagnostics.push(Diagnostic {
                                        code: "R0004",
                                        message: "invalid Unicode escape; expected `\\u{...}` with a valid Unicode scalar value".into(),
                                        span: Span {
                                            start: escape_start,
                                            end,
                                        },
                                        help: None,
                                    });
                                    if !recovering {
                                        break 'tokens;
                                    }
                                    i = skip_to_string_end(text, end);
                                    if i == bytes.len() {
                                        break 'tokens;
                                    }
                                    continue 'tokens;
                                }
                            }
                        }
                        value.push(match bytes[i] {
                            b'0' => '\0',
                            b'n' => '\n',
                            b'r' => '\r',
                            b't' => '\t',
                            b'"' => '"',
                            b'\\' => '\\',
                            _ => {
                                let invalid = text[i..].chars().next().unwrap_or('?');
                                diagnostics.push(Diagnostic {
                                    code: "R0004",
                                    message: "unsupported string escape".into(),
                                    span: Span {
                                        start: i - 1,
                                        end: i + invalid.len_utf8(),
                                    },
                                    help: None,
                                });
                                if !recovering {
                                    break 'tokens;
                                }
                                i = skip_to_string_end(text, i + invalid.len_utf8());
                                if i == bytes.len() {
                                    break 'tokens;
                                }
                                continue 'tokens;
                            }
                        });
                        i += 1;
                    } else {
                        let Some(ch) = text[i..].chars().next() else {
                            diagnostics.push(Diagnostic {
                                code: "R0002",
                                message: "invalid UTF-8 boundary".into(),
                                span: Span { start: i, end: i },
                                help: None,
                            });
                            break 'tokens;
                        };
                        value.push(ch);
                        i += ch.len_utf8();
                    }
                }
                if i == bytes.len() {
                    diagnostics.push(Diagnostic {
                        code: "R0003",
                        message: "unterminated string literal".into(),
                        span: Span { start, end: i },
                        help: None,
                    });
                    break 'tokens;
                }
                i += 1;
                TokenKind::String(value)
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                match &text[start..i] {
                    "fun" => TokenKind::Fn,
                    "mut" => TokenKind::Mut,
                    "true" => TokenKind::True,
                    "false" => TokenKind::False,
                    "when" => TokenKind::If,
                    "else" => TokenKind::Else,
                    "while" => TokenKind::While,
                    "for" => TokenKind::For,
                    "in" => TokenKind::In,
                    "break" => TokenKind::Break,
                    "continue" => TokenKind::Continue,
                    "return" => TokenKind::Return,
                    "echo" => TokenKind::Echo,
                    "struct" => TokenKind::Struct,
                    "as" => TokenKind::As,
                    "enum" => TokenKind::Enum,
                    "type" => TokenKind::TypeAlias,
                    "use" => TokenKind::Use,
                    "namespace" => TokenKind::Namespace,
                    "pub" => TokenKind::Pub,
                    "extend" => TokenKind::Extend,
                    "shape" => TokenKind::Shape,
                    "const" => TokenKind::Const,
                    "choose" => TokenKind::Choose,
                    "defer" => TokenKind::Defer,
                    other => TokenKind::Ident(other.into()),
                }
            }
            c if c.is_ascii_digit() => {
                let radix = if bytes[i] == b'0' {
                    match bytes.get(i + 1) {
                        Some(b'b' | b'B') => Some(2),
                        Some(b'x' | b'X') => Some(16),
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(radix) = radix {
                    match scan_radix_integer(text, bytes, start, &mut i, radix) {
                        Ok(value) => TokenKind::Integer(value),
                        Err(diagnostic) => {
                            diagnostics.push(diagnostic);
                            if !recovering {
                                break 'tokens;
                            }
                            continue;
                        }
                    }
                } else {
                    i += 1;
                    let mut invalid_separator = scan_decimal_digits(bytes, &mut i, true);
                    let mut is_float = false;
                    if bytes.get(i) == Some(&b'.')
                        && (bytes.get(i + 1).is_some_and(u8::is_ascii_digit)
                            || bytes.get(i + 1) == Some(&b'_'))
                    {
                        is_float = true;
                        i += 1;
                        let fraction_separator = scan_decimal_digits(bytes, &mut i, false);
                        if invalid_separator.is_none() {
                            invalid_separator = fraction_separator;
                        }
                    }
                    if bytes.get(i).is_some_and(|c| *c == b'e' || *c == b'E') {
                        is_float = true;
                        i += 1;
                        if bytes.get(i).is_some_and(|c| *c == b'+' || *c == b'-') {
                            i += 1;
                        }
                        let exponent_start = i;
                        let invalid_exponent_separator = scan_decimal_digits(bytes, &mut i, false);
                        invalid_separator = invalid_separator.or(invalid_exponent_separator);
                        if !bytes[exponent_start..i].iter().any(u8::is_ascii_digit) {
                            diagnostics.push(Diagnostic {
                                code: "R0007",
                                message: "expected exponent digits in floating-point literal"
                                    .into(),
                                span: Span { start, end: i },
                                help: None,
                            });
                            if !recovering {
                                break 'tokens;
                            }
                            continue;
                        }
                    }
                    if let Some(span) = invalid_separator {
                        diagnostics.push(numeric_separator_diagnostic(span));
                        if !recovering {
                            break 'tokens;
                        }
                        continue;
                    }
                    let numeric_text = &text[start..i];
                    let normalized = if numeric_text.contains('_') {
                        Cow::Owned(numeric_text.replace('_', ""))
                    } else {
                        Cow::Borrowed(numeric_text)
                    };
                    if is_float {
                        if let Some((diagnostic, resume)) = malformed_number_end(bytes, start, i) {
                            diagnostics.push(diagnostic);
                            if !recovering {
                                break 'tokens;
                            }
                            i = resume;
                            continue;
                        }
                        let value = match normalized.parse::<f64>() {
                            Ok(value) => value,
                            Err(_) => {
                                diagnostics.push(Diagnostic {
                                    code: "R0007",
                                    message:
                                        "floating-point literal is outside the supported f64 range"
                                            .into(),
                                    span: Span { start, end: i },
                                    help: None,
                                });
                                if !recovering {
                                    break 'tokens;
                                }
                                continue;
                            }
                        };
                        if !value.is_finite() {
                            diagnostics.push(Diagnostic {
                                code: "R0007",
                                message: "floating-point literal must be finite".into(),
                                span: Span { start, end: i },
                                help: None,
                            });
                            if !recovering {
                                break 'tokens;
                            }
                            continue;
                        }
                        TokenKind::Float(value)
                    } else {
                        let mut value = match normalized.parse::<u64>() {
                            Ok(value) => value,
                            Err(_) => {
                                diagnostics.push(Diagnostic {
                                    code: "R0006",
                                    message: "integer literal is outside the supported u64 range"
                                        .into(),
                                    span: Span { start, end: i },
                                    help: None,
                                });
                                if !recovering {
                                    break 'tokens;
                                }
                                continue;
                            }
                        };
                        let unit_start = i;
                        let mut unit_end = i;
                        while bytes.get(unit_end).is_some_and(u8::is_ascii_alphabetic) {
                            unit_end += 1;
                        }
                        let multiplier = match &text[unit_start..unit_end] {
                            "ms" => Some(1),
                            "s" => Some(1_000),
                            "min" => Some(60_000),
                            "h" => Some(3_600_000),
                            _ => None,
                        };
                        if let Some(multiplier) = multiplier {
                            let Some(milliseconds) = value.checked_mul(multiplier) else {
                                diagnostics.push(Diagnostic {
                                    code: "R0006",
                                    message:
                                        "duration literal exceeds the supported millisecond range"
                                            .into(),
                                    span: Span {
                                        start,
                                        end: unit_end,
                                    },
                                    help: Some("use a shorter duration".into()),
                                });
                                if !recovering {
                                    break 'tokens;
                                }
                                continue;
                            };
                            value = milliseconds;
                            i = unit_end;
                        }
                        if let Some((diagnostic, resume)) = malformed_number_end(bytes, start, i) {
                            diagnostics.push(diagnostic);
                            if !recovering {
                                break 'tokens;
                            }
                            i = resume;
                            continue;
                        }
                        TokenKind::Integer(value)
                    }
                }
            }
            _ => {
                let unexpected = text[i..].chars().next().unwrap_or('?');
                diagnostics.push(Diagnostic {
                    code: "R0005",
                    message: format!("unexpected character {unexpected:?}"),
                    span: Span {
                        start,
                        end: start + unexpected.len_utf8(),
                    },
                    help: None,
                });
                if !recovering {
                    break 'tokens;
                }
                i += unexpected.len_utf8();
                continue;
            }
        };
        out.push(Token {
            kind,
            span: Span { start, end: i },
        });
    }
    out.push(Token {
        kind: TokenKind::Eof,
        span: Span {
            start: text.len(),
            end: text.len(),
        },
    });
    (out, diagnostics)
}

fn skip_block_comment(bytes: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start + 2;
    let mut depth = 1_usize;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"/*") {
            depth += 1;
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"*/") {
            depth -= 1;
            cursor += 2;
            if depth == 0 {
                return Some(cursor);
            }
        } else {
            cursor += 1;
        }
    }
    None
}

fn scan_decimal_digits(bytes: &[u8], index: &mut usize, digit_before: bool) -> Option<Span> {
    let mut previous_was_digit = digit_before;
    let mut invalid_separator = None;
    while let Some(byte) = bytes.get(*index) {
        if byte.is_ascii_digit() {
            previous_was_digit = true;
            *index += 1;
        } else if *byte == b'_' {
            if (!previous_was_digit || !bytes.get(*index + 1).is_some_and(u8::is_ascii_digit))
                && invalid_separator.is_none()
            {
                invalid_separator = Some(Span {
                    start: *index,
                    end: *index + 1,
                });
            }
            previous_was_digit = false;
            *index += 1;
        } else {
            break;
        }
    }
    invalid_separator
}

fn scan_radix_integer(
    text: &str,
    bytes: &[u8],
    start: usize,
    index: &mut usize,
    radix: u32,
) -> Result<u64, Diagnostic> {
    *index += 2;
    let digits_start = *index;
    let mut digit_count = 0;
    let mut previous_was_digit = false;
    let mut invalid_separator = None;
    let mut invalid_digit = None;
    while let Some(byte) = bytes.get(*index).copied() {
        if byte == b'_' {
            let next_is_digit = bytes
                .get(*index + 1)
                .is_some_and(|next| radix_digit(*next, radix).is_some());
            if (!previous_was_digit || !next_is_digit) && invalid_separator.is_none() {
                invalid_separator = Some(Span {
                    start: *index,
                    end: *index + 1,
                });
            }
            previous_was_digit = false;
            *index += 1;
        } else if byte.is_ascii_alphanumeric() {
            if radix_digit(byte, radix).is_some() {
                digit_count += 1;
                previous_was_digit = true;
            } else {
                previous_was_digit = false;
                if invalid_digit.is_none() {
                    invalid_digit = Some(Diagnostic {
                        code: "R0009",
                        message: format!(
                            "digit `{}` is not valid in a base-{radix} integer literal",
                            char::from(byte)
                        ),
                        span: Span {
                            start: *index,
                            end: *index + 1,
                        },
                        help: Some(match radix {
                            2 => "use only `0` and `1` after the `0b` prefix".into(),
                            _ => "use hexadecimal digits `0`-`9`, `a`-`f`, or `A`-`F` after the `0x` prefix".into(),
                        }),
                    });
                }
            }
            *index += 1;
        } else {
            break;
        }
    }

    match (invalid_separator, invalid_digit) {
        (Some(separator), Some(digit)) if separator.start < digit.span.start => {
            return Err(numeric_separator_diagnostic(separator));
        }
        (Some(_), Some(digit)) => return Err(digit),
        (Some(separator), None) => return Err(numeric_separator_diagnostic(separator)),
        (None, Some(digit)) => return Err(digit),
        (None, None) => {}
    }

    if digit_count == 0 {
        return Err(Diagnostic {
            code: "R0009",
            message: format!("expected at least one base-{radix} digit after the prefix"),
            span: Span { start, end: *index },
            help: None,
        });
    }

    let digits = &text[digits_start..*index];
    let normalized = if digits.contains('_') {
        Cow::Owned(digits.replace('_', ""))
    } else {
        Cow::Borrowed(digits)
    };
    u64::from_str_radix(&normalized, radix).map_err(|_| Diagnostic {
        code: "R0006",
        message: "integer literal is outside the supported u64 range".into(),
        span: Span { start, end: *index },
        help: None,
    })
}

fn radix_digit(byte: u8, radix: u32) -> Option<u32> {
    char::from(byte).to_digit(radix)
}

fn numeric_separator_diagnostic(span: Span) -> Diagnostic {
    Diagnostic {
        code: "R0008",
        message: "numeric separators must appear between digits".into(),
        span,
        help: None,
    }
}

/// Reports a number that runs into identifier characters (`12abc`) or into a second decimal
/// point (`1.2.3`). `end` is the byte after the number; the returned byte is where lexing resumes.
fn malformed_number_end(bytes: &[u8], start: usize, end: usize) -> Option<(Diagnostic, usize)> {
    let is_name_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    if bytes.get(end).is_some_and(is_name_byte) {
        let mut run = end;
        while bytes.get(run).is_some_and(is_name_byte) {
            run += 1;
        }
        return Some((
            Diagnostic {
                code: "R0011",
                message: "numeric literal is directly followed by letters or `_`".into(),
                span: Span { start, end: run },
                help: Some("separate the number from the name with a space or an operator".into()),
            },
            run,
        ));
    }
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        let mut run = end + 1;
        while bytes.get(run).is_some_and(u8::is_ascii_digit) {
            run += 1;
        }
        return Some((
            Diagnostic {
                code: "R0011",
                message: "numeric literal is followed by another decimal point".into(),
                span: Span { start, end: run },
                help: Some(
                    "a floating-point literal has one `.`; separate values with spaces or operators"
                        .into(),
                ),
            },
            run,
        ));
    }
    None
}

fn character_literal(text: &str, start: usize) -> Result<(char, usize), Diagnostic> {
    let bytes = text.as_bytes();
    let mut at = start + 1;
    let parsed = if bytes.get(at) == Some(&b'\\') {
        at += 1;
        match bytes.get(at).copied() {
            Some(b'u') => decode_unicode_escape(text, at).ok().map(|(ch, end)| {
                at = end;
                ch
            }),
            Some(byte) => {
                at += 1;
                match byte {
                    b'n' => Some('\n'),
                    b'r' => Some('\r'),
                    b't' => Some('\t'),
                    b'0' => Some('\0'),
                    b'\\' => Some('\\'),
                    b'\'' => Some('\''),
                    b'"' => Some('"'),
                    _ => None,
                }
            }
            None => None,
        }
    } else if matches!(bytes.get(at), None | Some(b'\'' | b'\n' | b'\r')) {
        None
    } else {
        text[at..].chars().next().inspect(|ch| {
            at += ch.len_utf8();
        })
    };
    if let Some(character) = parsed
        && bytes.get(at) == Some(&b'\'')
    {
        return Ok((character, at + 1));
    }
    while at < bytes.len() && !matches!(bytes[at], b'\'' | b'\n' | b'\r') {
        at += 1;
    }
    if bytes.get(at) == Some(&b'\'') {
        at += 1;
    }
    Err(Diagnostic {
        code: "R0019",
        message: "character literal must contain exactly one Unicode scalar value".into(),
        span: Span {
            start,
            end: at.max(start + 1),
        },
        help: Some("use a single character such as 'a', '\\n', or '\\u{1F980}'".into()),
    })
}

fn skip_to_string_end(text: &str, mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len() {
        match bytes[at] {
            b'"' => return at + 1,
            b'\\' => {
                at += 1;
                if at < bytes.len() {
                    let ch = text[at..].chars().next().unwrap_or('?');
                    at += ch.len_utf8();
                }
            }
            _ => {
                let ch = text[at..].chars().next().unwrap_or('?');
                at += ch.len_utf8();
            }
        }
    }
    bytes.len()
}

fn decode_unicode_escape(text: &str, at: usize) -> Result<(char, usize), usize> {
    let bytes = text.as_bytes();
    if bytes.get(at + 1) != Some(&b'{') {
        return Err(at + 1);
    }

    let mut cursor = at + 2;
    let mut digits = 0;
    let mut value = 0_u32;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'}' => {
                if digits == 0 {
                    return Err(cursor + 1);
                }
                let Some(character) = char::from_u32(value) else {
                    return Err(cursor + 1);
                };
                return Ok((character, cursor + 1));
            }
            byte if byte.is_ascii_hexdigit() && digits < 6 => {
                let digit = match byte {
                    b'0'..=b'9' => u32::from(byte - b'0'),
                    b'a'..=b'f' => u32::from(byte - b'a' + 10),
                    _ => u32::from(byte - b'A' + 10),
                };
                value = value * 16 + digit;
                digits += 1;
                cursor += 1;
            }
            b'"' => return Err(cursor),
            byte if byte.is_ascii() => return Err(unicode_escape_end(bytes, cursor)),
            _ => {
                return Err(unicode_escape_end(bytes, cursor));
            }
        }
    }
    Err(cursor)
}

fn unicode_escape_end(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() {
        match bytes[at] {
            b'}' => return at + 1,
            b'"' => return at,
            _ => at += 1,
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::{TokenKind, lex, lex_recovering};

    #[test]
    fn lexes_comments_and_escaped_strings() {
        let tokens = lex("// note\necho(\"line\\nnext\\0end\")").expect("valid tokens");
        assert!(matches!(tokens[0].kind, TokenKind::Echo));
        assert!(matches!(&tokens[2].kind, TokenKind::String(s) if s == "line\nnext\0end"));
    }

    #[test]
    fn recognizes_only_the_canonical_declaration_and_control_keywords() {
        let tokens = lex("fun when mut value := 10 count: i32 = 2 fn let if print")
            .expect("removed spellings are ordinary identifiers");

        assert!(matches!(tokens[0].kind, TokenKind::Fn));
        assert!(matches!(tokens[1].kind, TokenKind::If));
        assert!(matches!(tokens[2].kind, TokenKind::Mut));
        assert!(matches!(tokens[4].kind, TokenKind::Define));
        assert!(matches!(tokens[7].kind, TokenKind::Colon));
        assert!(matches!(tokens[9].kind, TokenKind::Equal));
        assert!(matches!(&tokens[11].kind, TokenKind::Ident(name) if name == "fn"));
        assert!(matches!(&tokens[12].kind, TokenKind::Ident(name) if name == "let"));
        assert!(matches!(&tokens[13].kind, TokenKind::Ident(name) if name == "if"));
        assert!(matches!(&tokens[14].kind, TokenKind::Ident(name) if name == "print"));
    }

    #[test]
    fn lexes_nested_block_comments_without_treating_string_contents_as_comments() {
        let tokens = lex("/* outer /* nested */ still outer */ echo(\"/* text */\") /* tail */")
            .expect("nested block comments are valid");

        assert!(matches!(tokens[0].kind, TokenKind::Echo));
        assert!(matches!(&tokens[2].kind, TokenKind::String(value) if value == "/* text */"));
        assert!(matches!(tokens[4].kind, TokenKind::Eof));
    }

    #[test]
    fn unterminated_nested_block_comment_reports_its_full_span() {
        let source = "echo(1) /* outer /* nested */";
        let diagnostics =
            lex_recovering(source).expect_err("unterminated block comment is rejected");

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "R0018");
        assert_eq!(
            &source[diagnostics[0].span.start..diagnostics[0].span.end],
            "/* outer /* nested */"
        );
    }

    #[test]
    fn lexes_unicode_scalar_escapes() {
        let tokens = lex(r##"echo("\u{1F980}\u{E9}\u{0}")"##)
            .expect("valid Unicode scalar escapes are accepted");

        assert!(matches!(&tokens[2].kind, TokenKind::String(value) if value == "🦀é\0"));
    }

    #[test]
    fn rejects_invalid_unicode_scalar_escapes() {
        for escape in [
            r#"\u{}"#,
            r#"\u{xyz}"#,
            r#"\u{D800}"#,
            r#"\u{110000}"#,
            r#"\u{1234567}"#,
            r#"\u{123"#,
        ] {
            let source = format!("echo(\"{escape}\")");
            let diagnostic = lex(&source).expect_err("invalid Unicode escape is rejected");
            assert_eq!(diagnostic.code, "R0004", "source: {source:?}");
            assert!(diagnostic.message.contains("Unicode escape"));
            assert_eq!(
                &source[diagnostic.span.start..diagnostic.span.end],
                escape,
                "source: {source:?}"
            );
        }
    }

    #[test]
    fn unicode_escape_recovery_continues_after_the_string() {
        let diagnostics = lex_recovering("echo(\"\\u{D800}\") @")
            .expect_err("invalid Unicode escape and trailing token are rejected");

        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            ["R0004", "R0005"]
        );
    }

    #[test]
    fn line_comments_end_on_cr_lf_and_crlf() {
        let tokens = lex("// first\recho(1)\r\n// second\necho(2)")
            .expect("comments with all supported line endings are valid");

        assert!(matches!(tokens[0].kind, TokenKind::Echo));
        assert!(matches!(tokens[4].kind, TokenKind::Echo));
        assert!(matches!(tokens[8].kind, TokenKind::Eof));
    }

    #[test]
    fn reports_unterminated_strings() {
        let error = lex("echo(\"unfinished").expect_err("string should be closed");
        assert_eq!(error.code, "R0003");
    }

    #[test]
    fn lexes_decimal_and_exponent_float_literals() {
        let tokens = lex("echo(1.25) echo(2e3) echo(4.5E-1)").expect("valid floats");
        assert!(matches!(tokens[2].kind, TokenKind::Float(v) if (v - 1.25).abs() < f64::EPSILON));
        assert!(matches!(tokens[6].kind, TokenKind::Float(v) if v == 2000.0));
        assert!(matches!(tokens[10].kind, TokenKind::Float(v) if (v - 0.45).abs() < 1e-12));
    }

    #[test]
    fn lexes_numeric_separators_between_decimal_digits() {
        let tokens = lex("1_234 12_345.6_7e8_9").expect("separated literals are valid");
        assert!(matches!(tokens[0].kind, TokenKind::Integer(1234)));
        assert!(
            matches!(tokens[1].kind, TokenKind::Float(value) if (value - 12_345.67e89).abs() < 1e78)
        );
    }

    #[test]
    fn lexes_binary_and_hexadecimal_integer_literals() {
        let tokens =
            lex("0b1010 0B1_001 0xCA_FE 0Xff").expect("binary and hexadecimal literals are valid");
        assert!(matches!(tokens[0].kind, TokenKind::Integer(10)));
        assert!(matches!(tokens[1].kind, TokenKind::Integer(9)));
        assert!(matches!(tokens[2].kind, TokenKind::Integer(0xCAFE)));
        assert!(matches!(tokens[3].kind, TokenKind::Integer(255)));
    }

    #[test]
    fn rejects_invalid_binary_and_hexadecimal_literals() {
        let source = "0b 0x 0b2 0xG 0b1_2 0x_1 0x1_0000_0000_0000_0000";
        let diagnostics = lex_recovering(source)
            .expect_err("missing, malformed and overflowing radix literals are rejected");

        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                "R0009", "R0009", "R0009", "R0009", "R0008", "R0008", "R0006"
            ]
        );
        assert_eq!(
            &source[diagnostics[2].span.start..diagnostics[2].span.end],
            "2"
        );
        assert_eq!(
            &source[diagnostics[3].span.start..diagnostics[3].span.end],
            "G"
        );
    }

    #[test]
    fn rejects_numeric_separators_outside_decimal_digit_boundaries() {
        let source = "1__2 12_ 3.4_ 5e_2 6e+_2 7e2_ 8._2";
        let diagnostics =
            lex_recovering(source).expect_err("misplaced numeric separators are rejected");

        assert_eq!(diagnostics.len(), 7);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == "R0008")
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| &source[diagnostic.span.start..diagnostic.span.end])
                .collect::<Vec<_>>(),
            ["_", "_", "_", "_", "_", "_", "_"]
        );
    }

    #[test]
    fn rejects_non_finite_float_literals() {
        let error = lex("1e999").expect_err("infinite literal should be rejected");
        assert_eq!(error.code, "R0007");
    }

    #[test]
    fn rejects_numbers_running_into_names_or_a_second_decimal_point() {
        for source in ["12abc", "12i32", "1.2.3", "2s2"] {
            let error = lex(source).expect_err("malformed number should be rejected");
            assert_eq!(error.code, "R0011", "source: {source:?}");
        }
        let tokens = lex("3min 0..2 1.5e3").expect("valid number forms are lexable");
        assert!(matches!(tokens[0].kind, TokenKind::Integer(180_000)));
    }

    #[test]
    fn accepts_u64_maximum_and_rejects_larger_integer_tokens() {
        let tokens = lex("18446744073709551615").expect("u64 maximum is lexable");
        assert!(matches!(tokens[0].kind, TokenKind::Integer(u64::MAX)));
        let error = lex("18446744073709551616").expect_err("literal exceeds u64");
        assert_eq!(error.code, "R0006");
    }

    #[test]
    fn unexpected_unicode_character_span_covers_the_full_utf8_character() {
        let error = lex("💥").expect_err("emoji is not a valid Ryn token");

        assert_eq!(error.code, "R0005");
        assert_eq!(error.span.start, 0);
        assert_eq!(error.span.end, "💥".len());
    }

    #[test]
    fn lex_recovering_reports_multiple_unexpected_characters() {
        let source = "fun main() { @ echo(💥) }";
        let diagnostics = lex_recovering(source).expect_err("invalid characters are rejected");
        let first_error = lex(source).expect_err("the original lexer API returns its first error");

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(first_error.span.start, diagnostics[0].span.start);
        assert!(diagnostics.iter().all(|error| error.code == "R0005"));
        assert_eq!(
            &source[diagnostics[0].span.start..diagnostics[0].span.end],
            "@"
        );
        assert_eq!(
            &source[diagnostics[1].span.start..diagnostics[1].span.end],
            "💥"
        );
    }

    #[test]
    fn lex_recovering_skips_bad_literals_and_strings_before_continuing() {
        let diagnostics = lex_recovering("18446744073709551616 @ 1e999 💥 echo(\"bad\\q\") @")
            .expect_err("malformed literals and characters are rejected");

        assert_eq!(
            diagnostics
                .iter()
                .map(|error| error.code)
                .collect::<Vec<_>>(),
            ["R0006", "R0005", "R0007", "R0005", "R0004", "R0005"]
        );

        let source = "echo(\"bad\\💥 then \\\"still\\\"\") @";
        let diagnostics = lex_recovering(source)
            .expect_err("an invalid Unicode escape and trailing token should be reported");
        assert_eq!(
            diagnostics
                .iter()
                .map(|error| error.code)
                .collect::<Vec<_>>(),
            ["R0004", "R0005"]
        );
        assert_eq!(
            &source[diagnostics[0].span.start..diagnostics[0].span.end],
            "\\💥"
        );
    }

    #[test]
    fn lexes_compound_assignment_operators_without_splitting_them() {
        let tokens = lex("value += 1 value -= 2 value *= 3 value /= 4 value %= 5 value &= 6 value |= 7 value ^= 8 value <<= 9 value >>= 10")
            .expect("compound assignments are valid tokens");
        assert!(matches!(tokens[1].kind, TokenKind::PlusEqual));
        assert!(matches!(tokens[4].kind, TokenKind::MinusEqual));
        assert!(matches!(tokens[7].kind, TokenKind::StarEqual));
        assert!(matches!(tokens[10].kind, TokenKind::SlashEqual));
        assert!(matches!(tokens[13].kind, TokenKind::PercentEqual));
        assert!(matches!(tokens[16].kind, TokenKind::BitAndEqual));
        assert!(matches!(tokens[19].kind, TokenKind::BitOrEqual));
        assert!(matches!(tokens[22].kind, TokenKind::CaretEqual));
        assert!(matches!(tokens[25].kind, TokenKind::ShiftLeftEqual));
        assert!(matches!(tokens[28].kind, TokenKind::ShiftRightEqual));
    }

    #[test]
    fn lexes_bitwise_and_logical_operators_separately() {
        let tokens = lex("a & b | c ^ d ~e && f || g x << y >> z")
            .expect("bitwise and logical operators are valid tokens");

        assert!(matches!(tokens[1].kind, TokenKind::BitAnd));
        assert!(matches!(tokens[3].kind, TokenKind::BitOr));
        assert!(matches!(tokens[5].kind, TokenKind::Caret));
        assert!(matches!(tokens[7].kind, TokenKind::Tilde));
        assert!(matches!(tokens[9].kind, TokenKind::AndAnd));
        assert!(matches!(tokens[11].kind, TokenKind::OrOr));
        assert!(matches!(tokens[14].kind, TokenKind::ShiftLeft));
        assert!(matches!(tokens[16].kind, TokenKind::ShiftRight));
    }

    #[test]
    fn lexes_break_and_continue_keywords() {
        let tokens = lex("break continue").expect("loop control keywords are valid tokens");

        assert!(matches!(tokens[0].kind, TokenKind::Break));
        assert!(matches!(tokens[1].kind, TokenKind::Continue));
    }

    #[test]
    fn lexes_range_operator_without_changing_field_access() {
        let tokens = lex("for item in 1..2 { item.field }")
            .expect("range and field access tokens are valid");

        assert!(matches!(tokens[0].kind, TokenKind::For));
        assert!(matches!(tokens[1].kind, TokenKind::Ident(ref name) if name == "item"));
        assert!(matches!(tokens[2].kind, TokenKind::In));
        assert!(matches!(tokens[4].kind, TokenKind::DotDot));
        assert!(matches!(tokens[7].kind, TokenKind::Ident(ref name) if name == "item"));
        assert!(matches!(tokens[8].kind, TokenKind::Dot));
    }
}
