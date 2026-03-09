// Static type checker for Agentis.
//
// Runs before evaluation to catch type errors early.
// Supports: type inference for let bindings, structural typing for user types,
// mandatory annotations on function/agent signatures.

use std::collections::HashMap;

use crate::ast::*;

// --- Type representation ---

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    Void,
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Struct(String, Vec<(String, Type)>),
    /// A type variable that hasn't been resolved yet (for inference).
    Any,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::String => write!(f, "string"),
            Type::Bool => write!(f, "bool"),
            Type::Void => write!(f, "void"),
            Type::List(inner) => write!(f, "List<{inner}>"),
            Type::Map(k, v) => write!(f, "Map<{k}, {v}>"),
            Type::Struct(name, _) => write!(f, "{name}"),
            Type::Any => write!(f, "any"),
        }
    }
}

// --- Type errors ---

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub message: std::string::String,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "type error: {}", self.message)
    }
}

// --- Type environment ---

struct TypeEnv {
    scopes: Vec<HashMap<String, Type>>,
    functions: HashMap<String, (Vec<Type>, Type)>, // (param types, return type)
    types: HashMap<String, Vec<(String, Type)>>,   // struct definitions
}

impl TypeEnv {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            functions: HashMap::new(),
            types: HashMap::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Type) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<&Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    fn resolve_annotation(&self, ann: &TypeAnnotation) -> Type {
        match ann {
            TypeAnnotation::Named(name) => match name.as_str() {
                "int" => Type::Int,
                "float" => Type::Float,
                "string" => Type::String,
                "bool" => Type::Bool,
                "void" => Type::Void,
                other => {
                    if self.types.contains_key(other) {
                        Type::Struct(other.to_string(), self.types[other].clone())
                    } else {
                        Type::Any // unknown types degrade to Any
                    }
                }
            },
            TypeAnnotation::Generic(name, args) => {
                match name.as_str() {
                    "List" if args.len() == 1 => {
                        Type::List(Box::new(self.resolve_annotation(&args[0])))
                    }
                    "Map" if args.len() == 2 => Type::Map(
                        Box::new(self.resolve_annotation(&args[0])),
                        Box::new(self.resolve_annotation(&args[1])),
                    ),
                    _ => Type::Any,
                }
            }
        }
    }
}

// --- Type checker ---

pub struct TypeChecker {
    env: TypeEnv,
    errors: Vec<TypeError>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            env: TypeEnv::new(),
            errors: Vec::new(),
        }
    }

    /// Check a program and return all type errors found.
    pub fn check_program(&mut self, program: &Program) -> Vec<TypeError> {
        // First pass: collect type declarations and function signatures
        for decl in &program.declarations {
            match decl {
                Declaration::Type(td) => {
                    let fields: Vec<(String, Type)> = td
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), self.env.resolve_annotation(&f.type_annotation)))
                        .collect();
                    self.env.types.insert(td.name.clone(), fields);
                }
                Declaration::Function(f) => {
                    let params: Vec<Type> = f
                        .params
                        .iter()
                        .map(|p| self.env.resolve_annotation(&p.type_annotation))
                        .collect();
                    let ret = f
                        .return_type
                        .as_ref()
                        .map(|t| self.env.resolve_annotation(t))
                        .unwrap_or(Type::Void);
                    self.env.functions.insert(f.name.clone(), (params, ret));
                }
                Declaration::Agent(a) => {
                    let params: Vec<Type> = a
                        .params
                        .iter()
                        .map(|p| self.env.resolve_annotation(&p.type_annotation))
                        .collect();
                    let ret = a
                        .return_type
                        .as_ref()
                        .map(|t| self.env.resolve_annotation(t))
                        .unwrap_or(Type::Void);
                    self.env.functions.insert(a.name.clone(), (params, ret));
                }
                Declaration::Statement(_) => {}
                Declaration::Import(_) => {}
            }
        }

        // Second pass: check function bodies and top-level statements
        for decl in &program.declarations {
            match decl {
                Declaration::Function(f) => self.check_fn_body(f),
                Declaration::Agent(a) => self.check_agent_body(a),
                Declaration::Statement(stmt) => {
                    self.check_statement(stmt);
                }
                Declaration::Type(_) => {}
                Declaration::Import(_) => {}
            }
        }

        self.errors.clone()
    }

    fn check_fn_body(&mut self, f: &FnDecl) {
        self.env.push_scope();
        for p in &f.params {
            let ty = self.env.resolve_annotation(&p.type_annotation);
            self.env.define(&p.name, ty);
        }
        self.check_block(&f.body);
        self.env.pop_scope();
    }

    fn check_agent_body(&mut self, a: &AgentDecl) {
        self.env.push_scope();
        for p in &a.params {
            let ty = self.env.resolve_annotation(&p.type_annotation);
            self.env.define(&p.name, ty);
        }
        self.check_block(&a.body);
        self.env.pop_scope();
    }

    fn check_block(&mut self, block: &Block) {
        self.env.push_scope();
        for stmt in &block.statements {
            self.check_statement(stmt);
        }
        self.env.pop_scope();
    }

    fn check_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let(ls) => {
                let expr_type = self.infer_expr(&ls.value);
                if let Some(ann) = &ls.type_annotation {
                    let declared = self.env.resolve_annotation(ann);
                    if !self.types_compatible(&declared, &expr_type) {
                        self.errors.push(TypeError {
                            message: format!(
                                "cannot assign {} to variable '{}' of type {}",
                                expr_type, ls.name, declared
                            ),
                        });
                    }
                    self.env.define(&ls.name, declared);
                } else {
                    self.env.define(&ls.name, expr_type);
                }
            }
            Statement::Return(rs) => {
                if let Some(val) = &rs.value {
                    self.infer_expr(val);
                }
            }
            Statement::Expression(es) => {
                self.infer_expr(&es.expr);
            }
            Statement::Cb(_) => {}
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::IntLiteral(_) => Type::Int,
            Expr::FloatLiteral(_) => Type::Float,
            Expr::StringLiteral(_) => Type::String,
            Expr::BoolLiteral(_) => Type::Bool,
            Expr::Identifier(name) => {
                self.env.lookup(name).cloned().unwrap_or_else(|| {
                    // Don't error here — evaluator catches undefined vars
                    Type::Any
                })
            }
            Expr::Binary(b) => self.infer_binary(b),
            Expr::Unary(u) => self.infer_unary(u),
            Expr::Call(c) => self.infer_call(c),
            Expr::If(i) => {
                let cond_type = self.infer_expr(&i.condition);
                if !self.types_compatible(&Type::Bool, &cond_type)
                    && !self.types_compatible(&Type::Int, &cond_type)
                {
                    self.errors.push(TypeError {
                        message: format!("if condition must be bool or int, got {cond_type}"),
                    });
                }
                self.check_block(&i.then_block);
                if let Some(eb) = &i.else_block {
                    self.check_block(eb);
                }
                Type::Any // if-as-expression type is complex, degrade to Any
            }
            Expr::ListLiteral(items) => {
                if items.is_empty() {
                    Type::List(Box::new(Type::Any))
                } else {
                    let first = self.infer_expr(&items[0]);
                    for item in &items[1..] {
                        let t = self.infer_expr(item);
                        if !self.types_compatible(&first, &t) {
                            self.errors.push(TypeError {
                                message: format!(
                                    "list element type mismatch: expected {first}, got {t}"
                                ),
                            });
                        }
                    }
                    Type::List(Box::new(first))
                }
            }
            Expr::MapLiteral(entries) => {
                if entries.is_empty() {
                    Type::Map(Box::new(Type::Any), Box::new(Type::Any))
                } else {
                    let (first_k, first_v) = &entries[0];
                    let kt = self.infer_expr(first_k);
                    let vt = self.infer_expr(first_v);
                    for (k, v) in &entries[1..] {
                        self.infer_expr(k);
                        self.infer_expr(v);
                    }
                    Type::Map(Box::new(kt), Box::new(vt))
                }
            }
            Expr::FieldAccess(fa) => {
                let obj_type = self.infer_expr(&fa.object);
                match &obj_type {
                    Type::Struct(_, fields) => {
                        for (name, ty) in fields {
                            if name == &fa.field {
                                return ty.clone();
                            }
                        }
                        self.errors.push(TypeError {
                            message: format!(
                                "no field '{}' on type {obj_type}",
                                fa.field
                            ),
                        });
                        Type::Any
                    }
                    Type::Any => Type::Any,
                    _ => {
                        self.errors.push(TypeError {
                            message: format!(
                                "field access on non-struct type {obj_type}"
                            ),
                        });
                        Type::Any
                    }
                }
            }
            Expr::Prompt(p) => self.env.resolve_annotation(&p.return_type),
            Expr::Validate(v) => self.infer_expr(&v.target),
            Expr::Explore(_) => Type::Void,
            Expr::Spawn(s) => {
                for arg in &s.args {
                    self.infer_expr(arg);
                }
                Type::Any // agent handle
            }
        }
    }

    fn infer_binary(&mut self, expr: &BinaryExpr) -> Type {
        let left = self.infer_expr(&expr.left);
        let right = self.infer_expr(&expr.right);

        match expr.op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                match (&left, &right) {
                    (Type::Int, Type::Int) => Type::Int,
                    (Type::Float, Type::Float) => Type::Float,
                    (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
                    (Type::String, Type::String) if matches!(expr.op, BinaryOp::Add) => {
                        Type::String
                    }
                    (Type::Any, _) | (_, Type::Any) => Type::Any,
                    _ => {
                        self.errors.push(TypeError {
                            message: format!(
                                "cannot apply {:?} to {left} and {right}",
                                expr.op
                            ),
                        });
                        Type::Any
                    }
                }
            }
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Lt | BinaryOp::Gt
            | BinaryOp::LtEq | BinaryOp::GtEq => Type::Bool,
        }
    }

    fn infer_unary(&mut self, expr: &UnaryExpr) -> Type {
        let operand = self.infer_expr(&expr.operand);
        match expr.op {
            UnaryOp::Neg => operand,
            UnaryOp::Not => Type::Bool,
        }
    }

    fn infer_call(&mut self, expr: &CallExpr) -> Type {
        // Check arguments
        for arg in &expr.args {
            self.infer_expr(arg);
        }

        // Builtins
        match expr.callee.as_str() {
            "print" => Type::Void,
            "len" => Type::Int,
            "push" => {
                if let Some(first) = expr.args.first() {
                    self.infer_expr(first)
                } else {
                    Type::Any
                }
            }
            "get" => Type::Any,
            "map_of" => Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
            "typeof" => Type::String,
            _ => {
                // User-defined function
                if let Some((_, ret)) = self.env.functions.get(&expr.callee) {
                    ret.clone()
                } else {
                    Type::Any
                }
            }
        }
    }

    fn types_compatible(&self, expected: &Type, actual: &Type) -> bool {
        if *expected == Type::Any || *actual == Type::Any {
            return true;
        }
        expected == actual
    }
}

/// Convenience function: check a program and return errors.
pub fn check(program: &Program) -> Vec<TypeError> {
    let mut checker = TypeChecker::new();
    checker.check_program(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn check_source(source: &str) -> Vec<TypeError> {
        let program = Parser::parse_source(source).unwrap();
        check(&program)
    }

    #[test]
    fn valid_int_arithmetic() {
        assert!(check_source("let x: int = 1 + 2;").is_empty());
    }

    #[test]
    fn type_mismatch_let() {
        let errors = check_source("let x: int = \"hello\";");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("cannot assign"));
    }

    #[test]
    fn infer_let_type() {
        assert!(check_source("let x = 42; let y: int = x;").is_empty());
    }

    #[test]
    fn function_return_type() {
        assert!(check_source("fn add(a: int, b: int) -> int { return a + b; } let x: int = add(1, 2);").is_empty());
    }

    #[test]
    fn string_concat_valid() {
        assert!(check_source("let x: string = \"a\" + \"b\";").is_empty());
    }

    #[test]
    fn binary_type_mismatch() {
        let errors = check_source("let x = \"a\" - 1;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn list_homogeneous_valid() {
        assert!(check_source("let xs = [1, 2, 3];").is_empty());
    }

    #[test]
    fn list_heterogeneous_error() {
        let errors = check_source("let xs = [1, \"two\"];");
        assert!(!errors.is_empty());
    }

    #[test]
    fn field_access_valid() {
        // Struct type declared, variable has that type from function return
        assert!(check_source(
            "type Point { x: int, y: int } fn make() -> Point { let data = \"d\"; return prompt(\"mk\", data) -> Point; } let p = make(); let v = p.x;"
        ).is_empty());
    }

    #[test]
    fn field_access_invalid() {
        let errors = check_source("let x = 42; let y = x.foo;");
        assert!(!errors.is_empty());
    }

    #[test]
    fn if_condition_type() {
        assert!(check_source("if true { let x = 1; }").is_empty());
        assert!(check_source("if 1 { let x = 1; }").is_empty());
    }
}
