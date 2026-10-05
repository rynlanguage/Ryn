use crate::source::Span;

#[derive(Debug)]
pub struct Program {
    pub structs: Vec<StructDef>,
    pub functions: Vec<Function>,
}
#[derive(Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructField>,
    pub span: Span,
}
#[derive(Debug)]
pub struct StructField {
    pub name: String,
    pub ty: TypeName,
    pub span: Span,
}
#[derive(Debug)]
pub struct Function {
    pub name: String,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeName>,
    pub body: Vec<Statement>,
    pub return_value: Option<Expression>,
    pub span: Span,
}
#[derive(Debug)]
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
    Bool,
    Named(String, Span),
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

#[derive(Debug)]
pub enum Expression {
    Integer(u64, Span),
    Float(f64, Span),
    String(String, Span),
    Boolean(bool, Span),
    Name(String, Span),
    Call {
        name: String,
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
    Cast(Box<Expression>, TypeName, Span),
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Self::Integer(_, span)
            | Self::Float(_, span)
            | Self::String(_, span)
            | Self::Boolean(_, span)
            | Self::Name(_, span)
            | Self::Negate(_, span) => *span,
            Self::Not(_, span) | Self::BitNot(_, span) | Self::Cast(_, _, span) => *span,
            Self::Binary { span, .. } => *span,
            Self::Call { span, .. } => *span,
            Self::If { span, .. } => *span,
            Self::StructLiteral { span, .. } | Self::Field { span, .. } => *span,
        }
    }
}

#[derive(Debug)]
pub enum PrintPart {
    Text(String),
    Value(Expression),
}

#[derive(Debug)]
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
    Break(Span),
    Continue(Span),
    Return {
        value: Option<Expression>,
        span: Span,
    },
}
