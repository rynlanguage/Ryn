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
}

use crate::sema::Type;

impl VecOp {
    pub const ALL: [Self; 12] = [
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
    ];
    pub fn parameters(self, elem: Type) -> Vec<Type> {
        let vec_ty = Type::Vec(crate::sema::intern_vec_elem(elem));
        match self {
            Self::New => Vec::new(),
            Self::Clone | Self::Drop | Self::Len | Self::Capacity | Self::Clear => vec![vec_ty],
            Self::Reserve => vec![vec_ty, Type::U64],
            Self::Push => vec![vec_ty, elem],
            Self::Take | Self::Index | Self::Extract => vec![vec_ty, Type::U64],
            Self::Set => vec![vec_ty, Type::U64, elem],
        }
    }
    pub fn result(self, elem: Type) -> Option<Type> {
        match self {
            Self::New | Self::Clone => Some(Type::Vec(crate::sema::intern_vec_elem(elem))),
            Self::Len | Self::Capacity => Some(Type::U64),
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
        }
    }
    pub fn mutates(self) -> bool {
        matches!(
            self,
            Self::Reserve | Self::Clear | Self::Push | Self::Take | Self::Set | Self::Extract
        )
    }
    pub fn consumes_argument(self, index: usize) -> bool {
        (self == Self::Push && index == 1) || (self == Self::Set && index == 2)
    }
}
