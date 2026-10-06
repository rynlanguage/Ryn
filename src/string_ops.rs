use crate::sema::{Type, intern_vec_elem};

/// Native operations on the owning UTF-8 String type. Methods borrow their
/// receiver for the duration of the call; only returned Strings own new buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum StringOp {
    New,
    Clone,
    Drop,
    Print,
    Equals,
    Len,
    CharCount,
    ByteAt,
    CharAt,
    StrCharCount,
    StrCharAt,
    Slice,
    SliceChars,
    Data,
    Trim,
    Clear,
    Push,
    AppendStr,
    AppendString,
    ConcatStr,
    ConcatString,
    StartsWithStr,
    StartsWithString,
    EndsWithStr,
    EndsWithString,
    ContainsStr,
    ContainsString,
    FindStr,
    FindString,
    SplitStr,
    SplitString,
    PrintChar,
    FromI8,
    FromI16,
    FromI32,
    FromI64,
    FromU8,
    FromU16,
    FromU32,
    FromU64,
    FromF32,
    FromF64,
    ToI8,
    ToI16,
    ToI32,
    ToI64,
    ToU8,
    ToU16,
    ToU32,
    ToU64,
    ToF32,
    ToF64,
    TryParse,
    CharFromU32,
    IsEmpty,
    TrimStart,
    TrimEnd,
    ToLower,
    ToUpper,
    ReplaceStr,
    Lines,
    Chars,
    Bytes,
    Repeat,
    Reverse,
}

impl StringOp {
    pub const ALL: [Self; 65] = [
        Self::New,
        Self::Clone,
        Self::Drop,
        Self::Print,
        Self::Equals,
        Self::Len,
        Self::CharCount,
        Self::ByteAt,
        Self::CharAt,
        Self::StrCharCount,
        Self::StrCharAt,
        Self::Slice,
        Self::SliceChars,
        Self::Data,
        Self::Trim,
        Self::Clear,
        Self::Push,
        Self::AppendStr,
        Self::AppendString,
        Self::ConcatStr,
        Self::ConcatString,
        Self::StartsWithStr,
        Self::StartsWithString,
        Self::EndsWithStr,
        Self::EndsWithString,
        Self::ContainsStr,
        Self::ContainsString,
        Self::FindStr,
        Self::FindString,
        Self::SplitStr,
        Self::SplitString,
        Self::PrintChar,
        Self::FromI8,
        Self::FromI16,
        Self::FromI32,
        Self::FromI64,
        Self::FromU8,
        Self::FromU16,
        Self::FromU32,
        Self::FromU64,
        Self::FromF32,
        Self::FromF64,
        Self::ToI8,
        Self::ToI16,
        Self::ToI32,
        Self::ToI64,
        Self::ToU8,
        Self::ToU16,
        Self::ToU32,
        Self::ToU64,
        Self::ToF32,
        Self::ToF64,
        Self::TryParse,
        Self::CharFromU32,
        Self::IsEmpty,
        Self::TrimStart,
        Self::TrimEnd,
        Self::ToLower,
        Self::ToUpper,
        Self::ReplaceStr,
        Self::Lines,
        Self::Chars,
        Self::Bytes,
        Self::Repeat,
        Self::Reverse,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            Self::New => "ryn_string_new",
            Self::Clone => "ryn_string_clone",
            Self::Drop => "ryn_string_drop",
            Self::Print => "ryn_string_print",
            Self::Equals => "ryn_string_equals",
            Self::Len => "ryn_string_len",
            Self::CharCount => "ryn_string_char_count",
            Self::ByteAt => "ryn_string_byte_at",
            Self::CharAt => "ryn_string_char_at",
            Self::StrCharCount => "ryn_str_char_count",
            Self::StrCharAt => "ryn_str_char_at",
            Self::Slice => "ryn_string_slice",
            Self::SliceChars => "ryn_string_slice_chars",
            Self::Data => "ryn_string_data",
            Self::Trim => "ryn_string_trim",
            Self::Clear => "ryn_string_clear",
            Self::Push => "ryn_string_push",
            Self::AppendStr => "ryn_string_append_str",
            Self::AppendString => "ryn_string_append_string",
            Self::ConcatStr => "ryn_string_concat_str",
            Self::ConcatString => "ryn_string_concat_string",
            Self::StartsWithStr => "ryn_string_starts_with_str",
            Self::StartsWithString => "ryn_string_starts_with_string",
            Self::EndsWithStr => "ryn_string_ends_with_str",
            Self::EndsWithString => "ryn_string_ends_with_string",
            Self::ContainsStr => "ryn_string_contains_str",
            Self::ContainsString => "ryn_string_contains_string",
            Self::FindStr => "ryn_string_find_str",
            Self::FindString => "ryn_string_find_string",
            Self::SplitStr => "ryn_string_split_str",
            Self::SplitString => "ryn_string_split_string",
            Self::PrintChar => "ryn_print_char",
            Self::FromI8 => "ryn_string_from_i8",
            Self::FromI16 => "ryn_string_from_i16",
            Self::FromI32 => "ryn_string_from_i32",
            Self::FromI64 => "ryn_string_from_i64",
            Self::FromU8 => "ryn_string_from_u8",
            Self::FromU16 => "ryn_string_from_u16",
            Self::FromU32 => "ryn_string_from_u32",
            Self::FromU64 => "ryn_string_from_u64",
            Self::FromF32 => "ryn_string_from_f32",
            Self::FromF64 => "ryn_string_from_f64",
            Self::ToI8 => "ryn_string_to_i8",
            Self::ToI16 => "ryn_string_to_i16",
            Self::ToI32 => "ryn_string_to_i32",
            Self::ToI64 => "ryn_string_to_i64",
            Self::ToU8 => "ryn_string_to_u8",
            Self::ToU16 => "ryn_string_to_u16",
            Self::ToU32 => "ryn_string_to_u32",
            Self::ToU64 => "ryn_string_to_u64",
            Self::ToF32 => "ryn_string_to_f32",
            Self::ToF64 => "ryn_string_to_f64",
            Self::TryParse => "ryn_string_try_parse",
            Self::CharFromU32 => "ryn_char_from_u32",
            Self::IsEmpty => "ryn_string_is_empty",
            Self::TrimStart => "ryn_string_trim_start",
            Self::TrimEnd => "ryn_string_trim_end",
            Self::ToLower => "ryn_string_to_lower",
            Self::ToUpper => "ryn_string_to_upper",
            Self::ReplaceStr => "ryn_string_replace_str",
            Self::Lines => "ryn_string_lines",
            Self::Chars => "ryn_string_chars",
            Self::Bytes => "ryn_string_bytes",
            Self::Repeat => "ryn_string_repeat",
            Self::Reverse => "ryn_string_reverse",
        }
    }

    pub fn parameters(self) -> Vec<Type> {
        use Type::{Char, OwnedString as S, Str, U64};
        match self {
            Self::New => vec![Str],
            Self::CharFromU32 => vec![Type::U32],
            Self::PrintChar => vec![Char],
            Self::StrCharCount => vec![Str],
            Self::StrCharAt => vec![Str, U64],
            Self::ReplaceStr => vec![S, Str, Str],
            Self::Repeat => vec![S, U64],
            Self::Equals
            | Self::AppendString
            | Self::ConcatString
            | Self::StartsWithString
            | Self::EndsWithString
            | Self::ContainsString => vec![S, S],
            Self::FindString => vec![S, S],
            Self::SplitString => vec![S, S],
            Self::AppendStr
            | Self::ConcatStr
            | Self::StartsWithStr
            | Self::EndsWithStr
            | Self::ContainsStr => vec![S, Str],
            Self::FindStr => vec![S, Str],
            Self::SplitStr => vec![S, Str],
            Self::ByteAt | Self::CharAt => vec![S, U64],
            Self::Slice | Self::SliceChars => vec![S, U64, U64],
            Self::Data => vec![S],
            Self::Push => vec![S, Char],
            Self::FromI8 => vec![Type::I8],
            Self::FromI16 => vec![Type::I16],
            Self::FromI32 => vec![Type::I32],
            Self::FromI64 => vec![Type::I64],
            Self::FromU8 => vec![Type::U8],
            Self::FromU16 => vec![Type::U16],
            Self::FromU32 => vec![Type::U32],
            Self::FromU64 => vec![Type::U64],
            Self::FromF32 => vec![Type::F32],
            Self::FromF64 => vec![Type::F64],
            Self::TryParse => vec![S, Type::U32],
            _ => vec![S],
        }
    }

    pub fn result(self) -> Option<Type> {
        match self {
            Self::New
            | Self::Clone
            | Self::Slice
            | Self::SliceChars
            | Self::Data
            | Self::Trim
            | Self::TrimStart
            | Self::TrimEnd
            | Self::ToLower
            | Self::ToUpper
            | Self::ReplaceStr
            | Self::Repeat
            | Self::Reverse
            | Self::ConcatStr
            | Self::ConcatString => Some(Type::OwnedString),
            Self::Equals
            | Self::StartsWithStr
            | Self::StartsWithString
            | Self::EndsWithStr
            | Self::EndsWithString
            | Self::ContainsStr
            | Self::ContainsString => Some(Type::Bool),
            Self::FindStr | Self::FindString => Some(Type::I64),
            Self::SplitStr | Self::SplitString => {
                Some(Type::Vec(intern_vec_elem(Type::OwnedString)))
            }
            Self::Lines => Some(Type::Vec(intern_vec_elem(Type::OwnedString))),
            Self::Chars => Some(Type::Vec(intern_vec_elem(Type::Char))),
            Self::Bytes => Some(Type::Vec(intern_vec_elem(Type::U8))),
            Self::IsEmpty => Some(Type::Bool),
            Self::Len | Self::CharCount => Some(Type::U64),
            Self::StrCharCount => Some(Type::U64),
            Self::ByteAt => Some(Type::U8),
            Self::CharAt | Self::StrCharAt => Some(Type::Char),
            Self::CharFromU32 => Some(Type::Char),
            Self::FromI8
            | Self::FromI16
            | Self::FromI32
            | Self::FromI64
            | Self::FromU8
            | Self::FromU16
            | Self::FromU32
            | Self::FromU64
            | Self::FromF32
            | Self::FromF64 => Some(Type::OwnedString),
            Self::ToI8 => Some(Type::I8),
            Self::ToI16 => Some(Type::I16),
            Self::ToI32 => Some(Type::I32),
            Self::ToI64 => Some(Type::I64),
            Self::ToU8 => Some(Type::U8),
            Self::ToU16 => Some(Type::U16),
            Self::ToU32 => Some(Type::U32),
            Self::ToU64 => Some(Type::U64),
            Self::ToF32 => Some(Type::F32),
            Self::ToF64 => Some(Type::F64),
            Self::TryParse => Some(Type::Enum(0)),
            _ => None,
        }
    }

    pub fn from_numeric_type(ty: Type) -> Option<Self> {
        Some(match ty {
            Type::I8 => Self::FromI8,
            Type::I16 => Self::FromI16,
            Type::I32 => Self::FromI32,
            Type::I64 => Self::FromI64,
            Type::U8 => Self::FromU8,
            Type::U16 => Self::FromU16,
            Type::U32 => Self::FromU32,
            Type::U64 => Self::FromU64,
            Type::F32 => Self::FromF32,
            Type::F64 => Self::FromF64,
            _ => return None,
        })
    }

    pub fn parse_result(self) -> Option<Type> {
        match self {
            Self::ToI8 => Some(Type::I8),
            Self::ToI16 => Some(Type::I16),
            Self::ToI32 => Some(Type::I32),
            Self::ToI64 => Some(Type::I64),
            Self::ToU8 => Some(Type::U8),
            Self::ToU16 => Some(Type::U16),
            Self::ToU32 => Some(Type::U32),
            Self::ToU64 => Some(Type::U64),
            Self::ToF32 => Some(Type::F32),
            Self::ToF64 => Some(Type::F64),
            _ => None,
        }
    }

    pub fn mutates(self) -> bool {
        matches!(
            self,
            Self::AppendStr | Self::AppendString | Self::Push | Self::Clear
        )
    }
}
