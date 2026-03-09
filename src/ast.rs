// AST node types for the Agentis language.
//
// Every node supports manual binary serialization via `to_bytes`/`from_bytes`.
// This is critical: SHA-256 hashes are computed from these bytes, so the format
// must be stable and fully under our control.

// --- Binary format tag bytes ---
// Each node type gets a unique tag byte for deserialization dispatch.
const TAG_PROGRAM: u8 = 0x01;
const TAG_FN_DECL: u8 = 0x02;
const TAG_AGENT_DECL: u8 = 0x03;
const TAG_TYPE_DECL: u8 = 0x04;
const TAG_LET_STMT: u8 = 0x05;
const TAG_RETURN_STMT: u8 = 0x06;
const TAG_EXPR_STMT: u8 = 0x07;
const TAG_CB_STMT: u8 = 0x08;
const TAG_IF_EXPR: u8 = 0x10;
const TAG_CALL_EXPR: u8 = 0x11;
const TAG_BINARY_EXPR: u8 = 0x12;
const TAG_UNARY_EXPR: u8 = 0x13;
const TAG_IDENTIFIER: u8 = 0x14;
const TAG_INT_LITERAL: u8 = 0x15;
const TAG_FLOAT_LITERAL: u8 = 0x16;
const TAG_STRING_LITERAL: u8 = 0x17;
const TAG_BOOL_LITERAL: u8 = 0x18;
const TAG_EXPLORE_BLOCK: u8 = 0x20;
const TAG_PROMPT_EXPR: u8 = 0x21;
const TAG_VALIDATE_EXPR: u8 = 0x22;
const TAG_BLOCK: u8 = 0x30;
const TAG_TYPE_NAMED: u8 = 0x40;
const TAG_TYPE_GENERIC: u8 = 0x41;

// Binary operator tags
const OP_ADD: u8 = 0x01;
const OP_SUB: u8 = 0x02;
const OP_MUL: u8 = 0x03;
const OP_DIV: u8 = 0x04;
const OP_EQ: u8 = 0x05;
const OP_NOT_EQ: u8 = 0x06;
const OP_LT: u8 = 0x07;
const OP_GT: u8 = 0x08;
const OP_LT_EQ: u8 = 0x09;
const OP_GT_EQ: u8 = 0x0A;

// Unary operator tags
const UOP_NEG: u8 = 0x01;
const UOP_NOT: u8 = 0x02;

// --- AST Types ---

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Declaration {
    Function(FnDecl),
    Agent(AgentDecl),
    Type(TypeDecl),
    Statement(Statement),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeAnnotation>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeAnnotation>,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub fields: Vec<TypeField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeField {
    pub name: String,
    pub type_annotation: TypeAnnotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub type_annotation: TypeAnnotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Let(LetStmt),
    Return(ReturnStmt),
    Expression(ExprStmt),
    Cb(CbStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    pub name: String,
    pub type_annotation: Option<TypeAnnotation>,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprStmt {
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CbStmt {
    pub budget: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Identifier(String),
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(String),
    BoolLiteral(bool),
    Binary(Box<BinaryExpr>),
    Unary(Box<UnaryExpr>),
    Call(Box<CallExpr>),
    If(Box<IfExpr>),
    Prompt(Box<PromptExpr>),
    Validate(Box<ValidateExpr>),
    Explore(Box<ExploreBlock>),
    FieldAccess(Box<FieldAccessExpr>),
    ListLiteral(Vec<Expr>),
    MapLiteral(Vec<(Expr, Expr)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BinaryExpr {
    pub op: BinaryOp,
    pub left: Expr,
    pub right: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnaryExpr {
    pub op: UnaryOp,
    pub operand: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallExpr {
    pub callee: String,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    pub condition: Expr,
    pub then_block: Block,
    pub else_block: Option<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptExpr {
    pub instruction: String,
    pub input: Expr,
    pub return_type: TypeAnnotation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValidateExpr {
    pub target: Expr,
    pub predicates: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExploreBlock {
    pub name: String,
    pub body: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldAccessExpr {
    pub object: Expr,
    pub field: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeAnnotation {
    Named(String),
    Generic(String, Vec<TypeAnnotation>),
}

// --- Serialization Error ---

#[derive(Debug, Clone, PartialEq)]
pub struct SerError(pub String);

impl std::fmt::Display for SerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "serialization error: {}", self.0)
    }
}

// --- Binary serialization helpers ---

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bool(&mut self, v: bool) {
        self.buf.push(if v { 1 } else { 0 });
    }

    fn write_str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_u32(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
    }

    fn write_option<F>(&mut self, opt: &Option<impl Sized>, write_fn: F)
    where
        F: FnOnce(&mut Self),
    {
        if opt.is_some() {
            self.write_u8(1);
            write_fn(self);
        } else {
            self.write_u8(0);
        }
    }

    fn finish(self) -> Vec<u8> {
        self.buf
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn read_u8(&mut self) -> Result<u8, SerError> {
        if self.remaining() < 1 {
            return Err(SerError("unexpected end of data reading u8".into()));
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16, SerError> {
        if self.remaining() < 2 {
            return Err(SerError("unexpected end of data reading u16".into()));
        }
        let v = u16::from_le_bytes(self.data[self.pos..self.pos + 2].try_into().unwrap());
        self.pos += 2;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, SerError> {
        if self.remaining() < 4 {
            return Err(SerError("unexpected end of data reading u32".into()));
        }
        let v = u32::from_le_bytes(self.data[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn read_u64(&mut self) -> Result<u64, SerError> {
        if self.remaining() < 8 {
            return Err(SerError("unexpected end of data reading u64".into()));
        }
        let v = u64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    fn read_i64(&mut self) -> Result<i64, SerError> {
        if self.remaining() < 8 {
            return Err(SerError("unexpected end of data reading i64".into()));
        }
        let v = i64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    fn read_f64(&mut self) -> Result<f64, SerError> {
        if self.remaining() < 8 {
            return Err(SerError("unexpected end of data reading f64".into()));
        }
        let v = f64::from_le_bytes(self.data[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    fn read_bool(&mut self) -> Result<bool, SerError> {
        Ok(self.read_u8()? != 0)
    }

    fn read_str(&mut self) -> Result<String, SerError> {
        let len = self.read_u32()? as usize;
        if self.remaining() < len {
            return Err(SerError("unexpected end of data reading string".into()));
        }
        let s = std::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|e| SerError(format!("invalid UTF-8: {e}")))?
            .to_string();
        self.pos += len;
        Ok(s)
    }

    fn read_option<T, F>(&mut self, read_fn: F) -> Result<Option<T>, SerError>
    where
        F: FnOnce(&mut Self) -> Result<T, SerError>,
    {
        let present = self.read_u8()?;
        if present != 0 {
            Ok(Some(read_fn(self)?))
        } else {
            Ok(None)
        }
    }
}

// --- Serialization trait ---

pub trait Serialize: Sized {
    fn to_bytes(&self) -> Vec<u8>;
    fn from_bytes(data: &[u8]) -> Result<Self, SerError>;
}

// --- TypeAnnotation ---

impl TypeAnnotation {
    fn write(&self, w: &mut Writer) {
        match self {
            TypeAnnotation::Named(name) => {
                w.write_u8(TAG_TYPE_NAMED);
                w.write_str(name);
            }
            TypeAnnotation::Generic(name, params) => {
                w.write_u8(TAG_TYPE_GENERIC);
                w.write_str(name);
                w.write_u16(params.len() as u16);
                for p in params {
                    p.write(w);
                }
            }
        }
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            TAG_TYPE_NAMED => {
                let name = r.read_str()?;
                Ok(TypeAnnotation::Named(name))
            }
            TAG_TYPE_GENERIC => {
                let name = r.read_str()?;
                let count = r.read_u16()? as usize;
                let mut params = Vec::with_capacity(count);
                for _ in 0..count {
                    params.push(TypeAnnotation::read(r)?);
                }
                Ok(TypeAnnotation::Generic(name, params))
            }
            _ => Err(SerError(format!("unknown type annotation tag: 0x{tag:02X}"))),
        }
    }
}

impl Serialize for TypeAnnotation {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        TypeAnnotation::read(&mut r)
    }
}

// --- Param ---

impl Param {
    fn write(&self, w: &mut Writer) {
        w.write_str(&self.name);
        self.type_annotation.write(w);
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let name = r.read_str()?;
        let type_annotation = TypeAnnotation::read(r)?;
        Ok(Param { name, type_annotation })
    }
}

// --- TypeField ---

impl TypeField {
    fn write(&self, w: &mut Writer) {
        w.write_str(&self.name);
        self.type_annotation.write(w);
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let name = r.read_str()?;
        let type_annotation = TypeAnnotation::read(r)?;
        Ok(TypeField { name, type_annotation })
    }
}

// --- BinaryOp ---

impl BinaryOp {
    fn write(&self, w: &mut Writer) {
        let tag = match self {
            BinaryOp::Add => OP_ADD,
            BinaryOp::Sub => OP_SUB,
            BinaryOp::Mul => OP_MUL,
            BinaryOp::Div => OP_DIV,
            BinaryOp::Eq => OP_EQ,
            BinaryOp::NotEq => OP_NOT_EQ,
            BinaryOp::Lt => OP_LT,
            BinaryOp::Gt => OP_GT,
            BinaryOp::LtEq => OP_LT_EQ,
            BinaryOp::GtEq => OP_GT_EQ,
        };
        w.write_u8(tag);
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            OP_ADD => Ok(BinaryOp::Add),
            OP_SUB => Ok(BinaryOp::Sub),
            OP_MUL => Ok(BinaryOp::Mul),
            OP_DIV => Ok(BinaryOp::Div),
            OP_EQ => Ok(BinaryOp::Eq),
            OP_NOT_EQ => Ok(BinaryOp::NotEq),
            OP_LT => Ok(BinaryOp::Lt),
            OP_GT => Ok(BinaryOp::Gt),
            OP_LT_EQ => Ok(BinaryOp::LtEq),
            OP_GT_EQ => Ok(BinaryOp::GtEq),
            _ => Err(SerError(format!("unknown binary op tag: 0x{tag:02X}"))),
        }
    }
}

// --- UnaryOp ---

impl UnaryOp {
    fn write(&self, w: &mut Writer) {
        let tag = match self {
            UnaryOp::Neg => UOP_NEG,
            UnaryOp::Not => UOP_NOT,
        };
        w.write_u8(tag);
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            UOP_NEG => Ok(UnaryOp::Neg),
            UOP_NOT => Ok(UnaryOp::Not),
            _ => Err(SerError(format!("unknown unary op tag: 0x{tag:02X}"))),
        }
    }
}

// --- Expr ---

const TAG_EXPR_IDENTIFIER: u8 = 0x50;
const TAG_EXPR_INT: u8 = 0x51;
const TAG_EXPR_FLOAT: u8 = 0x52;
const TAG_EXPR_STRING: u8 = 0x53;
const TAG_EXPR_BOOL: u8 = 0x54;
const TAG_EXPR_BINARY: u8 = 0x55;
const TAG_EXPR_UNARY: u8 = 0x56;
const TAG_EXPR_CALL: u8 = 0x57;
const TAG_EXPR_IF: u8 = 0x58;
const TAG_EXPR_PROMPT: u8 = 0x59;
const TAG_EXPR_VALIDATE: u8 = 0x5A;
const TAG_EXPR_EXPLORE: u8 = 0x5B;
const TAG_EXPR_FIELD_ACCESS: u8 = 0x5C;
const TAG_EXPR_LIST: u8 = 0x5D;
const TAG_EXPR_MAP: u8 = 0x5E;

impl Expr {
    fn write(&self, w: &mut Writer) {
        match self {
            Expr::Identifier(name) => {
                w.write_u8(TAG_EXPR_IDENTIFIER);
                w.write_str(name);
            }
            Expr::IntLiteral(v) => {
                w.write_u8(TAG_EXPR_INT);
                w.write_i64(*v);
            }
            Expr::FloatLiteral(v) => {
                w.write_u8(TAG_EXPR_FLOAT);
                w.write_f64(*v);
            }
            Expr::StringLiteral(s) => {
                w.write_u8(TAG_EXPR_STRING);
                w.write_str(s);
            }
            Expr::BoolLiteral(v) => {
                w.write_u8(TAG_EXPR_BOOL);
                w.write_bool(*v);
            }
            Expr::Binary(b) => {
                w.write_u8(TAG_EXPR_BINARY);
                b.op.write(w);
                b.left.write(w);
                b.right.write(w);
            }
            Expr::Unary(u) => {
                w.write_u8(TAG_EXPR_UNARY);
                u.op.write(w);
                u.operand.write(w);
            }
            Expr::Call(c) => {
                w.write_u8(TAG_EXPR_CALL);
                w.write_str(&c.callee);
                w.write_u16(c.args.len() as u16);
                for arg in &c.args {
                    arg.write(w);
                }
            }
            Expr::If(i) => {
                w.write_u8(TAG_EXPR_IF);
                i.condition.write(w);
                write_block(&i.then_block, w);
                w.write_option(&i.else_block, |w| {
                    write_block(i.else_block.as_ref().unwrap(), w);
                });
            }
            Expr::Prompt(p) => {
                w.write_u8(TAG_EXPR_PROMPT);
                w.write_str(&p.instruction);
                p.input.write(w);
                p.return_type.write(w);
            }
            Expr::Validate(v) => {
                w.write_u8(TAG_EXPR_VALIDATE);
                v.target.write(w);
                w.write_u16(v.predicates.len() as u16);
                for pred in &v.predicates {
                    pred.write(w);
                }
            }
            Expr::Explore(e) => {
                w.write_u8(TAG_EXPR_EXPLORE);
                w.write_str(&e.name);
                write_block(&e.body, w);
            }
            Expr::FieldAccess(fa) => {
                w.write_u8(TAG_EXPR_FIELD_ACCESS);
                fa.object.write(w);
                w.write_str(&fa.field);
            }
            Expr::ListLiteral(items) => {
                w.write_u8(TAG_EXPR_LIST);
                w.write_u16(items.len() as u16);
                for item in items {
                    item.write(w);
                }
            }
            Expr::MapLiteral(entries) => {
                w.write_u8(TAG_EXPR_MAP);
                w.write_u16(entries.len() as u16);
                for (k, v) in entries {
                    k.write(w);
                    v.write(w);
                }
            }
        }
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            TAG_EXPR_IDENTIFIER => Ok(Expr::Identifier(r.read_str()?)),
            TAG_EXPR_INT => Ok(Expr::IntLiteral(r.read_i64()?)),
            TAG_EXPR_FLOAT => Ok(Expr::FloatLiteral(r.read_f64()?)),
            TAG_EXPR_STRING => Ok(Expr::StringLiteral(r.read_str()?)),
            TAG_EXPR_BOOL => Ok(Expr::BoolLiteral(r.read_bool()?)),
            TAG_EXPR_BINARY => {
                let op = BinaryOp::read(r)?;
                let left = Expr::read(r)?;
                let right = Expr::read(r)?;
                Ok(Expr::Binary(Box::new(BinaryExpr { op, left, right })))
            }
            TAG_EXPR_UNARY => {
                let op = UnaryOp::read(r)?;
                let operand = Expr::read(r)?;
                Ok(Expr::Unary(Box::new(UnaryExpr { op, operand })))
            }
            TAG_EXPR_CALL => {
                let callee = r.read_str()?;
                let count = r.read_u16()? as usize;
                let mut args = Vec::with_capacity(count);
                for _ in 0..count {
                    args.push(Expr::read(r)?);
                }
                Ok(Expr::Call(Box::new(CallExpr { callee, args })))
            }
            TAG_EXPR_IF => {
                let condition = Expr::read(r)?;
                let then_block = read_block(r)?;
                let else_block = r.read_option(read_block)?;
                Ok(Expr::If(Box::new(IfExpr { condition, then_block, else_block })))
            }
            TAG_EXPR_PROMPT => {
                let instruction = r.read_str()?;
                let input = Expr::read(r)?;
                let return_type = TypeAnnotation::read(r)?;
                Ok(Expr::Prompt(Box::new(PromptExpr { instruction, input, return_type })))
            }
            TAG_EXPR_VALIDATE => {
                let target = Expr::read(r)?;
                let count = r.read_u16()? as usize;
                let mut predicates = Vec::with_capacity(count);
                for _ in 0..count {
                    predicates.push(Expr::read(r)?);
                }
                Ok(Expr::Validate(Box::new(ValidateExpr { target, predicates })))
            }
            TAG_EXPR_EXPLORE => {
                let name = r.read_str()?;
                let body = read_block(r)?;
                Ok(Expr::Explore(Box::new(ExploreBlock { name, body })))
            }
            TAG_EXPR_FIELD_ACCESS => {
                let object = Expr::read(r)?;
                let field = r.read_str()?;
                Ok(Expr::FieldAccess(Box::new(FieldAccessExpr { object, field })))
            }
            TAG_EXPR_LIST => {
                let count = r.read_u16()? as usize;
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(Expr::read(r)?);
                }
                Ok(Expr::ListLiteral(items))
            }
            TAG_EXPR_MAP => {
                let count = r.read_u16()? as usize;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = Expr::read(r)?;
                    let v = Expr::read(r)?;
                    entries.push((k, v));
                }
                Ok(Expr::MapLiteral(entries))
            }
            _ => Err(SerError(format!("unknown expr tag: 0x{tag:02X}"))),
        }
    }
}

impl Serialize for Expr {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        Expr::read(&mut r)
    }
}

// --- Statement ---

const TAG_STMT_LET: u8 = 0x60;
const TAG_STMT_RETURN: u8 = 0x61;
const TAG_STMT_EXPR: u8 = 0x62;
const TAG_STMT_CB: u8 = 0x63;

impl Statement {
    fn write(&self, w: &mut Writer) {
        match self {
            Statement::Let(l) => {
                w.write_u8(TAG_STMT_LET);
                w.write_str(&l.name);
                w.write_option(&l.type_annotation, |w| {
                    l.type_annotation.as_ref().unwrap().write(w);
                });
                l.value.write(w);
            }
            Statement::Return(ret) => {
                w.write_u8(TAG_STMT_RETURN);
                w.write_option(&ret.value, |w| {
                    ret.value.as_ref().unwrap().write(w);
                });
            }
            Statement::Expression(e) => {
                w.write_u8(TAG_STMT_EXPR);
                e.expr.write(w);
            }
            Statement::Cb(cb) => {
                w.write_u8(TAG_STMT_CB);
                w.write_u64(cb.budget);
            }
        }
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            TAG_STMT_LET => {
                let name = r.read_str()?;
                let type_annotation = r.read_option(TypeAnnotation::read)?;
                let value = Expr::read(r)?;
                Ok(Statement::Let(LetStmt { name, type_annotation, value }))
            }
            TAG_STMT_RETURN => {
                let value = r.read_option(Expr::read)?;
                Ok(Statement::Return(ReturnStmt { value }))
            }
            TAG_STMT_EXPR => {
                let expr = Expr::read(r)?;
                Ok(Statement::Expression(ExprStmt { expr }))
            }
            TAG_STMT_CB => {
                let budget = r.read_u64()?;
                Ok(Statement::Cb(CbStmt { budget }))
            }
            _ => Err(SerError(format!("unknown statement tag: 0x{tag:02X}"))),
        }
    }
}

impl Serialize for Statement {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        Statement::read(&mut r)
    }
}

// --- Block ---

fn write_block(block: &Block, w: &mut Writer) {
    w.write_u8(TAG_BLOCK);
    w.write_u16(block.statements.len() as u16);
    for stmt in &block.statements {
        stmt.write(w);
    }
}

fn read_block(r: &mut Reader) -> Result<Block, SerError> {
    let tag = r.read_u8()?;
    if tag != TAG_BLOCK {
        return Err(SerError(format!("expected block tag 0x{TAG_BLOCK:02X}, got 0x{tag:02X}")));
    }
    let count = r.read_u16()? as usize;
    let mut statements = Vec::with_capacity(count);
    for _ in 0..count {
        statements.push(Statement::read(r)?);
    }
    Ok(Block { statements })
}

impl Serialize for Block {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        write_block(self, &mut w);
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        read_block(&mut r)
    }
}

// --- Declaration ---

const TAG_DECL_FN: u8 = 0x70;
const TAG_DECL_AGENT: u8 = 0x71;
const TAG_DECL_TYPE: u8 = 0x72;
const TAG_DECL_STMT: u8 = 0x73;

impl Declaration {
    fn write(&self, w: &mut Writer) {
        match self {
            Declaration::Function(f) => {
                w.write_u8(TAG_DECL_FN);
                w.write_str(&f.name);
                w.write_u16(f.params.len() as u16);
                for p in &f.params {
                    p.write(w);
                }
                w.write_option(&f.return_type, |w| {
                    f.return_type.as_ref().unwrap().write(w);
                });
                write_block(&f.body, w);
            }
            Declaration::Agent(a) => {
                w.write_u8(TAG_DECL_AGENT);
                w.write_str(&a.name);
                w.write_u16(a.params.len() as u16);
                for p in &a.params {
                    p.write(w);
                }
                w.write_option(&a.return_type, |w| {
                    a.return_type.as_ref().unwrap().write(w);
                });
                write_block(&a.body, w);
            }
            Declaration::Type(t) => {
                w.write_u8(TAG_DECL_TYPE);
                w.write_str(&t.name);
                w.write_u16(t.fields.len() as u16);
                for f in &t.fields {
                    f.write(w);
                }
            }
            Declaration::Statement(s) => {
                w.write_u8(TAG_DECL_STMT);
                s.write(w);
            }
        }
    }

    fn read(r: &mut Reader) -> Result<Self, SerError> {
        let tag = r.read_u8()?;
        match tag {
            TAG_DECL_FN => {
                let name = r.read_str()?;
                let param_count = r.read_u16()? as usize;
                let mut params = Vec::with_capacity(param_count);
                for _ in 0..param_count {
                    params.push(Param::read(r)?);
                }
                let return_type = r.read_option(TypeAnnotation::read)?;
                let body = read_block(r)?;
                Ok(Declaration::Function(FnDecl { name, params, return_type, body }))
            }
            TAG_DECL_AGENT => {
                let name = r.read_str()?;
                let param_count = r.read_u16()? as usize;
                let mut params = Vec::with_capacity(param_count);
                for _ in 0..param_count {
                    params.push(Param::read(r)?);
                }
                let return_type = r.read_option(TypeAnnotation::read)?;
                let body = read_block(r)?;
                Ok(Declaration::Agent(AgentDecl { name, params, return_type, body }))
            }
            TAG_DECL_TYPE => {
                let name = r.read_str()?;
                let field_count = r.read_u16()? as usize;
                let mut fields = Vec::with_capacity(field_count);
                for _ in 0..field_count {
                    fields.push(TypeField::read(r)?);
                }
                Ok(Declaration::Type(TypeDecl { name, fields }))
            }
            TAG_DECL_STMT => {
                let stmt = Statement::read(r)?;
                Ok(Declaration::Statement(stmt))
            }
            _ => Err(SerError(format!("unknown declaration tag: 0x{tag:02X}"))),
        }
    }
}

impl Serialize for Declaration {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.write(&mut w);
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        Declaration::read(&mut r)
    }
}

// --- Program ---

impl Serialize for Program {
    fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u8(TAG_PROGRAM);
        w.write_u16(self.declarations.len() as u16);
        for decl in &self.declarations {
            decl.write(&mut w);
        }
        w.finish()
    }

    fn from_bytes(data: &[u8]) -> Result<Self, SerError> {
        let mut r = Reader::new(data);
        let tag = r.read_u8()?;
        if tag != TAG_PROGRAM {
            return Err(SerError(format!("expected program tag 0x{TAG_PROGRAM:02X}, got 0x{tag:02X}")));
        }
        let count = r.read_u16()? as usize;
        let mut declarations = Vec::with_capacity(count);
        for _ in 0..count {
            declarations.push(Declaration::read(&mut r)?);
        }
        Ok(Program { declarations })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: serialize then deserialize, assert equality.
    fn round_trip<T: Serialize + PartialEq + std::fmt::Debug>(value: &T) {
        let bytes = value.to_bytes();
        let recovered = T::from_bytes(&bytes).unwrap();
        assert_eq!(*value, recovered);
    }

    // --- TypeAnnotation ---

    #[test]
    fn type_named() {
        round_trip(&TypeAnnotation::Named("int".into()));
    }

    #[test]
    fn type_generic() {
        round_trip(&TypeAnnotation::Generic(
            "list".into(),
            vec![TypeAnnotation::Named("int".into())],
        ));
    }

    #[test]
    fn type_nested_generic() {
        round_trip(&TypeAnnotation::Generic(
            "map".into(),
            vec![
                TypeAnnotation::Named("string".into()),
                TypeAnnotation::Generic("list".into(), vec![TypeAnnotation::Named("int".into())]),
            ],
        ));
    }

    // --- Expr ---

    #[test]
    fn expr_identifier() {
        round_trip(&Expr::Identifier("foo".into()));
    }

    #[test]
    fn expr_int_literal() {
        round_trip(&Expr::IntLiteral(42));
        round_trip(&Expr::IntLiteral(-1));
        round_trip(&Expr::IntLiteral(0));
        round_trip(&Expr::IntLiteral(i64::MAX));
        round_trip(&Expr::IntLiteral(i64::MIN));
    }

    #[test]
    fn expr_float_literal() {
        round_trip(&Expr::FloatLiteral(3.14));
        round_trip(&Expr::FloatLiteral(0.0));
        round_trip(&Expr::FloatLiteral(-1.5));
    }

    #[test]
    fn expr_string_literal() {
        round_trip(&Expr::StringLiteral("hello world".into()));
        round_trip(&Expr::StringLiteral(String::new()));
        round_trip(&Expr::StringLiteral("unicode: 日本語".into()));
    }

    #[test]
    fn expr_bool_literal() {
        round_trip(&Expr::BoolLiteral(true));
        round_trip(&Expr::BoolLiteral(false));
    }

    #[test]
    fn expr_binary() {
        round_trip(&Expr::Binary(Box::new(BinaryExpr {
            op: BinaryOp::Add,
            left: Expr::IntLiteral(1),
            right: Expr::IntLiteral(2),
        })));
    }

    #[test]
    fn expr_all_binary_ops() {
        let ops = [
            BinaryOp::Add, BinaryOp::Sub, BinaryOp::Mul, BinaryOp::Div,
            BinaryOp::Eq, BinaryOp::NotEq, BinaryOp::Lt, BinaryOp::Gt,
            BinaryOp::LtEq, BinaryOp::GtEq,
        ];
        for op in ops {
            round_trip(&Expr::Binary(Box::new(BinaryExpr {
                op,
                left: Expr::IntLiteral(1),
                right: Expr::IntLiteral(2),
            })));
        }
    }

    #[test]
    fn expr_unary() {
        round_trip(&Expr::Unary(Box::new(UnaryExpr {
            op: UnaryOp::Neg,
            operand: Expr::IntLiteral(5),
        })));
        round_trip(&Expr::Unary(Box::new(UnaryExpr {
            op: UnaryOp::Not,
            operand: Expr::BoolLiteral(true),
        })));
    }

    #[test]
    fn expr_call() {
        round_trip(&Expr::Call(Box::new(CallExpr {
            callee: "print".into(),
            args: vec![Expr::StringLiteral("hello".into()), Expr::IntLiteral(42)],
        })));
    }

    #[test]
    fn expr_call_no_args() {
        round_trip(&Expr::Call(Box::new(CallExpr {
            callee: "noop".into(),
            args: vec![],
        })));
    }

    #[test]
    fn expr_if_no_else() {
        round_trip(&Expr::If(Box::new(IfExpr {
            condition: Expr::BoolLiteral(true),
            then_block: Block {
                statements: vec![Statement::Return(ReturnStmt {
                    value: Some(Expr::IntLiteral(1)),
                })],
            },
            else_block: None,
        })));
    }

    #[test]
    fn expr_if_with_else() {
        round_trip(&Expr::If(Box::new(IfExpr {
            condition: Expr::Binary(Box::new(BinaryExpr {
                op: BinaryOp::Gt,
                left: Expr::Identifier("x".into()),
                right: Expr::IntLiteral(0),
            })),
            then_block: Block {
                statements: vec![Statement::Return(ReturnStmt {
                    value: Some(Expr::StringLiteral("positive".into())),
                })],
            },
            else_block: Some(Block {
                statements: vec![Statement::Return(ReturnStmt {
                    value: Some(Expr::StringLiteral("non-positive".into())),
                })],
            }),
        })));
    }

    #[test]
    fn expr_prompt() {
        round_trip(&Expr::Prompt(Box::new(PromptExpr {
            instruction: "Classify this text".into(),
            input: Expr::Identifier("data".into()),
            return_type: TypeAnnotation::Named("Category".into()),
        })));
    }

    #[test]
    fn expr_validate() {
        round_trip(&Expr::Validate(Box::new(ValidateExpr {
            target: Expr::Identifier("result".into()),
            predicates: vec![
                Expr::Binary(Box::new(BinaryExpr {
                    op: BinaryOp::Gt,
                    left: Expr::FieldAccess(Box::new(FieldAccessExpr {
                        object: Expr::Identifier("result".into()),
                        field: "confidence".into(),
                    })),
                    right: Expr::FloatLiteral(0.8),
                })),
            ],
        })));
    }

    #[test]
    fn expr_explore() {
        round_trip(&Expr::Explore(Box::new(ExploreBlock {
            name: "feature-x".into(),
            body: Block {
                statements: vec![Statement::Expression(ExprStmt {
                    expr: Expr::Call(Box::new(CallExpr {
                        callee: "print".into(),
                        args: vec![Expr::StringLiteral("exploring".into())],
                    })),
                })],
            },
        })));
    }

    #[test]
    fn expr_field_access() {
        round_trip(&Expr::FieldAccess(Box::new(FieldAccessExpr {
            object: Expr::Identifier("result".into()),
            field: "confidence".into(),
        })));
    }

    #[test]
    fn expr_nested_field_access() {
        round_trip(&Expr::FieldAccess(Box::new(FieldAccessExpr {
            object: Expr::FieldAccess(Box::new(FieldAccessExpr {
                object: Expr::Identifier("a".into()),
                field: "b".into(),
            })),
            field: "c".into(),
        })));
    }

    // --- Statement ---

    #[test]
    fn stmt_let_with_type() {
        round_trip(&Statement::Let(LetStmt {
            name: "x".into(),
            type_annotation: Some(TypeAnnotation::Named("int".into())),
            value: Expr::IntLiteral(42),
        }));
    }

    #[test]
    fn stmt_let_without_type() {
        round_trip(&Statement::Let(LetStmt {
            name: "x".into(),
            type_annotation: None,
            value: Expr::IntLiteral(42),
        }));
    }

    #[test]
    fn stmt_return_with_value() {
        round_trip(&Statement::Return(ReturnStmt {
            value: Some(Expr::IntLiteral(0)),
        }));
    }

    #[test]
    fn stmt_return_empty() {
        round_trip(&Statement::Return(ReturnStmt { value: None }));
    }

    #[test]
    fn stmt_expr() {
        round_trip(&Statement::Expression(ExprStmt {
            expr: Expr::Call(Box::new(CallExpr {
                callee: "print".into(),
                args: vec![Expr::StringLiteral("hello".into())],
            })),
        }));
    }

    #[test]
    fn stmt_cb() {
        round_trip(&Statement::Cb(CbStmt { budget: 1000 }));
    }

    // --- Block ---

    #[test]
    fn empty_block() {
        round_trip(&Block { statements: vec![] });
    }

    #[test]
    fn block_with_statements() {
        round_trip(&Block {
            statements: vec![
                Statement::Let(LetStmt {
                    name: "x".into(),
                    type_annotation: Some(TypeAnnotation::Named("int".into())),
                    value: Expr::IntLiteral(10),
                }),
                Statement::Return(ReturnStmt {
                    value: Some(Expr::Identifier("x".into())),
                }),
            ],
        });
    }

    // --- Declaration ---

    #[test]
    fn decl_function() {
        round_trip(&Declaration::Function(FnDecl {
            name: "add".into(),
            params: vec![
                Param { name: "a".into(), type_annotation: TypeAnnotation::Named("int".into()) },
                Param { name: "b".into(), type_annotation: TypeAnnotation::Named("int".into()) },
            ],
            return_type: Some(TypeAnnotation::Named("int".into())),
            body: Block {
                statements: vec![Statement::Return(ReturnStmt {
                    value: Some(Expr::Binary(Box::new(BinaryExpr {
                        op: BinaryOp::Add,
                        left: Expr::Identifier("a".into()),
                        right: Expr::Identifier("b".into()),
                    }))),
                })],
            },
        }));
    }

    #[test]
    fn decl_function_no_return_type() {
        round_trip(&Declaration::Function(FnDecl {
            name: "noop".into(),
            params: vec![],
            return_type: None,
            body: Block { statements: vec![] },
        }));
    }

    #[test]
    fn decl_agent() {
        round_trip(&Declaration::Agent(AgentDecl {
            name: "scanner".into(),
            params: vec![
                Param { name: "url".into(), type_annotation: TypeAnnotation::Named("string".into()) },
            ],
            return_type: Some(TypeAnnotation::Named("Report".into())),
            body: Block {
                statements: vec![
                    Statement::Cb(CbStmt { budget: 1000 }),
                    Statement::Return(ReturnStmt {
                        value: Some(Expr::Prompt(Box::new(PromptExpr {
                            instruction: "Analyze this page".into(),
                            input: Expr::Identifier("url".into()),
                            return_type: TypeAnnotation::Named("Report".into()),
                        }))),
                    }),
                ],
            },
        }));
    }

    #[test]
    fn decl_type() {
        round_trip(&Declaration::Type(TypeDecl {
            name: "Category".into(),
            fields: vec![
                TypeField {
                    name: "label".into(),
                    type_annotation: TypeAnnotation::Named("string".into()),
                },
                TypeField {
                    name: "confidence".into(),
                    type_annotation: TypeAnnotation::Named("float".into()),
                },
            ],
        }));
    }

    #[test]
    fn decl_type_with_generic_field() {
        round_trip(&Declaration::Type(TypeDecl {
            name: "Dataset".into(),
            fields: vec![
                TypeField {
                    name: "items".into(),
                    type_annotation: TypeAnnotation::Generic(
                        "list".into(),
                        vec![TypeAnnotation::Named("string".into())],
                    ),
                },
            ],
        }));
    }

    // --- Program ---

    #[test]
    fn program_empty() {
        round_trip(&Program { declarations: vec![] });
    }

    #[test]
    fn program_full() {
        round_trip(&Program {
            declarations: vec![
                Declaration::Type(TypeDecl {
                    name: "Report".into(),
                    fields: vec![
                        TypeField {
                            name: "summary".into(),
                            type_annotation: TypeAnnotation::Named("string".into()),
                        },
                        TypeField {
                            name: "score".into(),
                            type_annotation: TypeAnnotation::Named("float".into()),
                        },
                    ],
                }),
                Declaration::Function(FnDecl {
                    name: "add".into(),
                    params: vec![
                        Param { name: "a".into(), type_annotation: TypeAnnotation::Named("int".into()) },
                        Param { name: "b".into(), type_annotation: TypeAnnotation::Named("int".into()) },
                    ],
                    return_type: Some(TypeAnnotation::Named("int".into())),
                    body: Block {
                        statements: vec![Statement::Return(ReturnStmt {
                            value: Some(Expr::Binary(Box::new(BinaryExpr {
                                op: BinaryOp::Add,
                                left: Expr::Identifier("a".into()),
                                right: Expr::Identifier("b".into()),
                            }))),
                        })],
                    },
                }),
                Declaration::Agent(AgentDecl {
                    name: "analyzer".into(),
                    params: vec![
                        Param { name: "data".into(), type_annotation: TypeAnnotation::Named("string".into()) },
                    ],
                    return_type: Some(TypeAnnotation::Named("Report".into())),
                    body: Block {
                        statements: vec![
                            Statement::Cb(CbStmt { budget: 500 }),
                            Statement::Let(LetStmt {
                                name: "result".into(),
                                type_annotation: None,
                                value: Expr::Prompt(Box::new(PromptExpr {
                                    instruction: "Analyze this".into(),
                                    input: Expr::Identifier("data".into()),
                                    return_type: TypeAnnotation::Named("Report".into()),
                                })),
                            }),
                            Statement::Expression(ExprStmt {
                                expr: Expr::Validate(Box::new(ValidateExpr {
                                    target: Expr::Identifier("result".into()),
                                    predicates: vec![
                                        Expr::Binary(Box::new(BinaryExpr {
                                            op: BinaryOp::Gt,
                                            left: Expr::FieldAccess(Box::new(FieldAccessExpr {
                                                object: Expr::Identifier("result".into()),
                                                field: "score".into(),
                                            })),
                                            right: Expr::FloatLiteral(0.5),
                                        })),
                                    ],
                                })),
                            }),
                            Statement::Return(ReturnStmt {
                                value: Some(Expr::Identifier("result".into())),
                            }),
                        ],
                    },
                }),
            ],
        });
    }

    // --- Error cases ---

    #[test]
    fn invalid_tag() {
        let result = Expr::from_bytes(&[0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn truncated_data() {
        let result = Expr::from_bytes(&[TAG_EXPR_INT, 0x01]);
        assert!(result.is_err());
    }

    #[test]
    fn empty_data() {
        let result = Program::from_bytes(&[]);
        assert!(result.is_err());
    }

    // --- Determinism ---

    #[test]
    fn serialization_is_deterministic() {
        let program = Program {
            declarations: vec![
                Declaration::Function(FnDecl {
                    name: "test".into(),
                    params: vec![Param {
                        name: "x".into(),
                        type_annotation: TypeAnnotation::Named("int".into()),
                    }],
                    return_type: Some(TypeAnnotation::Named("int".into())),
                    body: Block {
                        statements: vec![Statement::Return(ReturnStmt {
                            value: Some(Expr::Identifier("x".into())),
                        })],
                    },
                }),
            ],
        };
        let bytes1 = program.to_bytes();
        let bytes2 = program.to_bytes();
        assert_eq!(bytes1, bytes2, "serialization must be deterministic");
    }

    #[test]
    fn serialize_list_literal_empty() {
        round_trip(&Expr::ListLiteral(vec![]));
    }

    #[test]
    fn serialize_list_literal_items() {
        round_trip(&Expr::ListLiteral(vec![
            Expr::IntLiteral(1),
            Expr::StringLiteral("hello".into()),
            Expr::BoolLiteral(true),
        ]));
    }

    #[test]
    fn serialize_map_literal() {
        round_trip(&Expr::MapLiteral(vec![
            (Expr::StringLiteral("key".into()), Expr::IntLiteral(42)),
            (Expr::IntLiteral(1), Expr::BoolLiteral(false)),
        ]));
    }
}
