use crate::source::Span;

#[derive(Debug)]
pub struct Program {
    pub uses: Vec<UseDecl>,
    pub structs: Vec<StructDef>,
    pub enums: Vec<EnumDef>,
    pub type_aliases: Vec<TypeAliasDef>,
    pub extends: Vec<ExtendDef>,
    pub shapes: Vec<ShapeDef>,
    pub functions: Vec<Function>,
}

#[derive(Clone, Debug)]
pub struct ShapeDef {
    pub name: String,
    pub methods: Vec<ShapeMethod>,
    pub module_path: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct ShapeMethod {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeName>,
    pub default_body: Option<Vec<Statement>>,
    pub default_value: Option<Expression>,
    pub span: Span,
}
#[derive(Debug)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct TypeAliasDef {
    pub name: String,
    pub type_parameters: Vec<String>,
    pub ty: TypeName,
    pub public: bool,
    pub module_path: String,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub type_parameters: Vec<String>,
    pub variants: Vec<VariantDef>,
    pub public: bool,
    pub module_path: String,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<TypeName>,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct StructDef {
    pub name: String,
    pub type_parameters: Vec<String>,
    pub fields: Vec<StructField>,
    pub public: bool,
    pub repr_c: bool,
    pub drop_function: Option<String>,
    pub derives: Vec<String>,
    pub module_path: String,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct StructField {
    pub name: String,
    pub ty: TypeName,
    pub public: bool,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct ExtendDef {
    pub type_name: TypeName,
    pub as_shape: Option<String>,
    pub functions: Vec<Function>,
    pub constants: Vec<ConstantDef>,
    pub module_path: String,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct ConstantDef {
    pub name: String,
    pub ty: TypeName,
    pub value: Expression,
    pub public: bool,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub extern_c: bool,
    pub external_symbol: Option<String>,
    pub type_parameters: Vec<String>,
    pub type_parameter_bounds: Vec<(String, Vec<String>)>,
    pub public: bool,
    pub module_path: String,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeName>,
    pub body: Vec<Statement>,
    pub return_value: Option<Expression>,
    pub span: Span,
}
#[derive(Clone, Debug)]
pub struct Parameter {
    pub name: String,
    pub ty: TypeName,
    pub span: Span,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeName {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
    OwnedString,
    Char,
    Bool,
    Named(String, Span),
    Parameter(String, Span),
    Vec(Box<TypeName>, Span),
    Map(Box<TypeName>, Box<TypeName>, Span),
    Set(Box<TypeName>, Span),
    Array(Box<TypeName>, usize, Span),
    Slice(Box<TypeName>, Span),
    Reference(Box<TypeName>, bool, Span),
    RawPointer(Box<TypeName>, Span),
    FunctionPointer(Vec<TypeName>, Option<Box<TypeName>>, bool, Span),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    BitAnd,
    BitXor,
    BitOr,
    ShiftLeft,
    ShiftRight,
    And,
    Or,
}

#[derive(Clone, Debug)]
pub enum Expression {
    Integer(u64, Span),
    Float(f64, Span),
    String(String, Span),
    Character(char, Span),
    Boolean(bool, Span),
    Name(String, Span),
    Call {
        name: String,
        type_arguments: Vec<TypeName>,
        arguments: Vec<Expression>,
        span: Span,
    },
    MethodCall {
        value: Box<Expression>,
        name: String,
        type_arguments: Vec<TypeName>,
        arguments: Vec<Expression>,
        span: Span,
    },
    StructLiteral {
        name: String,
        fields: Vec<(String, Expression, Span)>,
        span: Span,
    },
    Field {
        value: Box<Expression>,
        name: String,
        name_span: Span,
        span: Span,
    },
    If {
        condition: Box<Expression>,
        then_value: Box<Expression>,
        else_value: Box<Expression>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expression>,
        right: Box<Expression>,
        span: Span,
    },
    Negate(Box<Expression>, Span),
    Not(Box<Expression>, Span),
    BitNot(Box<Expression>, Span),
    AddressOf {
        mutable: bool,
        raw: bool,
        value: Box<Expression>,
        span: Span,
    },
    Dereference(Box<Expression>, Span),
    Cast(Box<Expression>, TypeName, Span),
    LayoutOf {
        ty: TypeName,
        alignment: bool,
        span: Span,
    },
    VecConstructor {
        element: TypeName,
        span: Span,
    },
    MapConstructor {
        key: TypeName,
        value: TypeName,
        span: Span,
    },
    SetConstructor {
        element: TypeName,
        span: Span,
    },
    Tuple(Vec<Expression>, Span),
    ArrayLiteral(Vec<Expression>, Span),
    Index {
        value: Box<Expression>,
        index: Box<Expression>,
        span: Span,
    },
    Propagate(Box<Expression>, Span),
    EnumConstruct {
        enum_name: String,
        variant: String,
        arguments: Vec<Expression>,
        span: Span,
    },
    Choose {
        value: Box<Expression>,
        arms: Vec<ChooseArm>,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub struct ChooseArm {
    pub enum_name: Option<String>,
    pub variant: Option<String>,
    pub bindings: Vec<String>,
    pub body: Expression,
    pub span: Span,
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Self::Integer(_, span)
            | Self::Float(_, span)
            | Self::String(_, span)
            | Self::Character(_, span)
            | Self::Boolean(_, span)
            | Self::Name(_, span)
            | Self::Negate(_, span) => *span,
            Self::Not(_, span)
            | Self::BitNot(_, span)
            | Self::Dereference(_, span)
            | Self::Cast(_, _, span) => *span,
            Self::AddressOf { span, .. } => *span,
            Self::LayoutOf { span, .. } => *span,
            Self::Binary { span, .. } => *span,
            Self::Call { span, .. } => *span,
            Self::MethodCall { span, .. } => *span,
            Self::If { span, .. } => *span,
            Self::StructLiteral { span, .. } | Self::Field { span, .. } => *span,
            Self::VecConstructor { span, .. } => *span,
            Self::MapConstructor { span, .. } => *span,
            Self::SetConstructor { span, .. } => *span,
            Self::Tuple(_, span) => *span,
            Self::ArrayLiteral(_, span) => *span,
            Self::Index { span, .. } => *span,
            Self::Propagate(_, span) => *span,
            Self::EnumConstruct { span, .. } | Self::Choose { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug)]
pub enum PrintPart {
    Text(String),
    Value(Expression),
}

#[derive(Clone, Debug)]
pub enum Statement {
    Let {
        name: String,
        mutable: bool,
        annotation: Option<TypeName>,
        value: Expression,
        span: Span,
    },
    Assign {
        name: String,
        value: Expression,
        span: Span,
    },
    IndexAssign {
        name: String,
        index: Expression,
        value: Expression,
        span: Span,
    },
    DereferenceAssign {
        pointer: Expression,
        value: Expression,
        span: Span,
    },
    FieldAssign {
        object: String,
        fields: Vec<(String, Span)>,
        op: Option<BinaryOp>,
        value: Expression,
        span: Span,
    },
    CompoundAssign {
        name: String,
        op: BinaryOp,
        value: Expression,
        span: Span,
    },
    Print(Expression, Span),
    PrintTemplate(Vec<PrintPart>, Span),
    Call {
        name: String,
        type_arguments: Vec<TypeName>,
        arguments: Vec<Expression>,
        span: Span,
    },
    MethodCall {
        value: Box<Expression>,
        name: String,
        arguments: Vec<Expression>,
        span: Span,
    },
    If {
        condition: Expression,
        then_body: Vec<Statement>,
        else_body: Vec<Statement>,
        span: Span,
    },
    While {
        condition: Expression,
        body: Vec<Statement>,
        span: Span,
    },
    For {
        name: String,
        name_span: Span,
        start: Expression,
        end: Expression,
        body: Vec<Statement>,
        span: Span,
    },
    ForEach {
        name: String,
        name_span: Span,
        collection: Expression,
        body: Vec<Statement>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Return {
        value: Option<Expression>,
        span: Span,
    },
}
