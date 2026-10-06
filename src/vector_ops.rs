#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum VecOp {
    New,
    Clone,
    Drop,
    Len,
    Capacity,
    Reserve,
    Clear,
    Push,
    Take,
    Index,
    Set,
    Extract,
    IsEmpty,
    Insert,
    Reverse,
    Sort,
    Contains,
    GetOption,
    PopOption,
}

use crate::sema::Type;

impl VecOp {
    pub const ALL: [Self; 19] = [
        Self::New,
        Self::Clone,
        Self::Drop,
        Self::Len,
        Self::Capacity,
        Self::Reserve,
        Self::Clear,
        Self::Push,
        Self::Take,
        Self::Index,
        Self::Set,
        Self::Extract,
        Self::IsEmpty,
        Self::Insert,
        Self::Reverse,
        Self::Sort,
        Self::Contains,
        Self::GetOption,
        Self::PopOption,
    ];
    pub fn parameters(self, elem: Type) -> Vec<Type> {
        let vec_ty = Type::Vec(crate::sema::intern_vec_elem(elem));
        match self {
            Self::New => Vec::new(),
            Self::Clone
            | Self::Drop
            | Self::Len
            | Self::Capacity
            | Self::Clear
            | Self::IsEmpty
            | Self::Reverse
            | Self::Sort => {
                vec![vec_ty]
            }
            Self::Reserve => vec![vec_ty, Type::U64],
            Self::Push | Self::Contains => vec![vec_ty, elem],
            Self::GetOption => vec![
                vec_ty,
                Type::U64,
                Type::RawPointer(crate::sema::intern_pointer_target(elem)),
            ],
            Self::PopOption => vec![
                vec_ty,
                Type::RawPointer(crate::sema::intern_pointer_target(elem)),
            ],
            Self::Take | Self::Index | Self::Extract => vec![vec_ty, Type::U64],
            Self::Set => vec![vec_ty, Type::U64, elem],
            Self::Insert => vec![vec_ty, Type::U64, elem],
        }
    }
    pub fn result(self, elem: Type) -> Option<Type> {
        match self {
            Self::New | Self::Clone => Some(Type::Vec(crate::sema::intern_vec_elem(elem))),
            Self::Len | Self::Capacity => Some(Type::U64),
            Self::IsEmpty | Self::Contains => Some(Type::Bool),
            Self::Take | Self::Index | Self::Extract => Some(elem),
            _ => None,
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Self::New => "ryn_vec_new",
            Self::Clone => "ryn_vec_clone",
            Self::Drop => "ryn_vec_drop",
            Self::Len => "ryn_vec_len",
            Self::Capacity => "ryn_vec_capacity",
            Self::Reserve => "ryn_vec_reserve",
            Self::Clear => "ryn_vec_clear",
            Self::Push => "ryn_vec_push",
            Self::Take => "ryn_vec_take",
            Self::Index => "ryn_vec_index",
            Self::Set => "ryn_vec_set",
            Self::Extract => "ryn_vec_extract",
            Self::IsEmpty => "ryn_vec_is_empty",
            Self::Insert => "ryn_vec_insert",
            Self::Reverse => "ryn_vec_reverse",
            Self::Sort => "ryn_vec_sort",
            Self::Contains => "ryn_vec_contains",
            Self::GetOption => "ryn_vec_get_option",
            Self::PopOption => "ryn_vec_pop_option",
        }
    }
    pub fn mutates(self) -> bool {
        matches!(
            self,
            Self::Reserve
                | Self::Clear
                | Self::Push
                | Self::Take
                | Self::Set
                | Self::Extract
                | Self::Insert
                | Self::Reverse
                | Self::Sort
                | Self::PopOption
        )
    }
    pub fn consumes_argument(self, index: usize) -> bool {
        (self == Self::Push && index == 1)
            || (self == Self::Set && index == 2)
            || (self == Self::Insert && index == 2)
    }
}
