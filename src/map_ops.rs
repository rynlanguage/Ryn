use crate::sema::Type;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum MapOp {
    New,
    Drop,
    Clone,
    Len,
    Clear,
    ContainsKey,
    Insert,
    Get,
    Remove,
    Keys,
    Values,
}

impl MapOp {
    pub const ALL: [Self; 11] = [
        Self::New,
        Self::Drop,
        Self::Clone,
        Self::Len,
        Self::Clear,
        Self::ContainsKey,
        Self::Insert,
        Self::Get,
        Self::Remove,
        Self::Keys,
        Self::Values,
    ];

    pub fn parameters(self, key: Type, value: Type) -> Vec<Type> {
        let map = Type::Map(crate::sema::intern_map(key, value));
        match self {
            Self::New => Vec::new(),
            Self::Drop | Self::Clone | Self::Len | Self::Clear => vec![map],
            Self::ContainsKey | Self::Remove => vec![map, key],
            Self::Insert => vec![map, key, value],
            Self::Get => vec![map, key],
            Self::Keys | Self::Values => vec![map],
        }
    }

    pub fn result(self, key: Type, value: Type) -> Option<Type> {
        match self {
            Self::New | Self::Clone => Some(Type::Map(crate::sema::intern_map(key, value))),
            Self::Len => Some(Type::U64),
            Self::ContainsKey | Self::Insert | Self::Remove => Some(Type::Bool),
            Self::Get => None,
            Self::Drop | Self::Clear => None,
            Self::Keys => Some(Type::Vec(crate::sema::intern_vec_elem(key))),
            Self::Values => Some(Type::Vec(crate::sema::intern_vec_elem(value))),
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::New => "ryn_map_new",
            Self::Drop => "ryn_map_drop",
            Self::Clone => "ryn_map_clone",
            Self::Len => "ryn_map_len",
            Self::Clear => "ryn_map_clear",
            Self::ContainsKey => "ryn_map_contains_key",
            Self::Insert => "ryn_map_insert",
            Self::Get => "ryn_map_get",
            Self::Remove => "ryn_map_remove",
            Self::Keys => "ryn_map_keys",
            Self::Values => "ryn_map_values",
        }
    }

    pub fn mutates(self) -> bool {
        matches!(self, Self::Clear | Self::Insert | Self::Remove)
    }

    pub fn consumes_argument(self, index: usize) -> bool {
        (self == Self::Insert && matches!(index, 1 | 2)) || (self == Self::New && index == 0)
    }
}
