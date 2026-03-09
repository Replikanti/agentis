use std::collections::HashMap;

use crate::ast::*;
use crate::capabilities::{CapError, CapKind, CapabilityRegistry};
use crate::llm::LlmBackend;
use crate::refs::Refs;
use crate::snapshot::{MemorySnapshot, SnapshotManager};
use crate::storage::ObjectStore;

// --- Runtime Values ---

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Struct(String, HashMap<String, Value>),
    Void,
}

impl Value {
    pub fn type_name(&self) -> &str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Struct(name, _) => name,
            Value::Void => "void",
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::String(s) => !s.is_empty(),
            Value::List(items) => !items.is_empty(),
            _ => true,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => write!(f, "{n}"),
            Value::String(s) => write!(f, "{s}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Map(entries) => {
                write!(f, "{{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::Struct(name, fields) => {
                write!(f, "{name} {{ ")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            }
            Value::Void => write!(f, "void"),
        }
    }
}

// --- Errors ---

#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    CognitiveOverload { budget: u64, required: u64 },
    ValidationFailed { predicate_index: usize, detail: String },
    UndefinedVariable(String),
    UndefinedFunction(String),
    UndefinedType(String),
    UndefinedField { type_name: String, field: String },
    TypeError { expected: String, got: String },
    DivisionByZero,
    ArityMismatch { expected: usize, got: usize },
    Return(Value),
    NotAStruct(String),
    CapabilityDenied(CapError),
    General(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::CognitiveOverload { budget, required } => {
                write!(f, "CognitiveOverload: budget {budget}, required {required}")
            }
            EvalError::ValidationFailed { predicate_index, detail } => {
                write!(f, "ValidationFailed: predicate #{predicate_index}: {detail}")
            }
            EvalError::UndefinedVariable(name) => write!(f, "undefined variable: {name}"),
            EvalError::UndefinedFunction(name) => write!(f, "undefined function: {name}"),
            EvalError::UndefinedType(name) => write!(f, "undefined type: {name}"),
            EvalError::UndefinedField { type_name, field } => {
                write!(f, "undefined field '{field}' on type '{type_name}'")
            }
            EvalError::TypeError { expected, got } => {
                write!(f, "type error: expected {expected}, got {got}")
            }
            EvalError::DivisionByZero => write!(f, "division by zero"),
            EvalError::ArityMismatch { expected, got } => {
                write!(f, "arity mismatch: expected {expected} args, got {got}")
            }
            EvalError::Return(_) => write!(f, "return outside function"),
            EvalError::NotAStruct(t) => write!(f, "field access on non-struct type: {t}"),
            EvalError::CapabilityDenied(e) => write!(f, "capability denied: {e}"),
            EvalError::General(msg) => write!(f, "{msg}"),
        }
    }
}

// --- Callable ---

#[derive(Debug, Clone)]
enum Callable {
    Function(FnDecl),
    Agent(AgentDecl),
}

// --- Environment ---

#[derive(Debug, Clone)]
struct Environment {
    scopes: Vec<HashMap<String, Value>>,
}

impl Environment {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(v) = scope.get(name) {
                return Some(v);
            }
        }
        None
    }

    fn snapshot_scopes(&self) -> Vec<HashMap<String, Value>> {
        self.scopes.clone()
    }

    fn restore_scopes(&mut self, scopes: Vec<HashMap<String, Value>>) {
        self.scopes = scopes;
    }
}

// --- Evaluator ---

pub struct Evaluator<'a> {
    env: Environment,
    functions: HashMap<String, Callable>,
    types: HashMap<String, TypeDecl>,
    budget: u64,
    output: Vec<String>,
    vcs: Option<(&'a ObjectStore, &'a Refs)>,
    explore_branches: Vec<String>,
    cap_registry: CapabilityRegistry,
    caps: HashMap<CapKind, Vec<crate::capabilities::CapHandle>>,
    snapshot_mgr: Option<SnapshotManager<'a>>,
    llm_backend: Option<&'a dyn LlmBackend>,
}

impl<'a> Evaluator<'a> {
    pub fn new(budget: u64) -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            types: HashMap::new(),
            budget,
            output: Vec::new(),
            vcs: None,
            explore_branches: Vec::new(),
            cap_registry: CapabilityRegistry::new(),
            caps: HashMap::new(),
            snapshot_mgr: None,
            llm_backend: None,
        }
    }

    pub fn with_vcs(mut self, store: &'a ObjectStore, refs: &'a Refs) -> Self {
        self.vcs = Some((store, refs));
        self
    }

    pub fn with_llm(mut self, backend: &'a dyn LlmBackend) -> Self {
        self.llm_backend = Some(backend);
        self
    }

    pub fn with_persistence(mut self, store: &'a ObjectStore) -> Self {
        self.snapshot_mgr = Some(SnapshotManager::new(store));
        self
    }

    pub fn grant(&mut self, kind: CapKind) {
        let handle = self.cap_registry.grant(kind);
        self.caps.entry(kind).or_default().push(handle);
    }

    pub fn grant_all(&mut self) {
        for kind in CapKind::all() {
            self.grant(*kind);
        }
    }

    pub fn revoke(&mut self, kind: CapKind) {
        if let Some(handles) = self.caps.remove(&kind) {
            for h in &handles {
                self.cap_registry.revoke(h);
            }
        }
    }

    fn require_cap(&self, kind: CapKind) -> Result<(), EvalError> {
        match self.caps.get(&kind) {
            Some(handles) => {
                for h in handles {
                    if self.cap_registry.check(h, kind).is_ok() {
                        return Ok(());
                    }
                }
                Err(EvalError::CapabilityDenied(CapError::RevokedCapability(kind)))
            }
            None => Err(EvalError::CapabilityDenied(
                CapError::MissingCapability(kind),
            )),
        }
    }

    pub fn capture_snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            scopes: self.env.snapshot_scopes(),
            budget_remaining: self.budget,
            output: self.output.clone(),
        }
    }

    pub fn restore_snapshot(&mut self, snapshot: &MemorySnapshot) {
        self.env.restore_scopes(snapshot.scopes.clone());
        self.budget = snapshot.budget_remaining;
        self.output = snapshot.output.clone();
    }

    fn persist_snapshot(&mut self) {
        if let Some(ref mut mgr) = self.snapshot_mgr {
            let snap = MemorySnapshot {
                scopes: self.env.snapshot_scopes(),
                budget_remaining: self.budget,
                output: self.output.clone(),
            };
            let _ = mgr.save(&snap);
        }
    }

    pub fn snapshot_history(&self) -> &[crate::storage::Hash] {
        match &self.snapshot_mgr {
            Some(mgr) => mgr.history(),
            None => &[],
        }
    }

    pub fn budget_remaining(&self) -> u64 {
        self.budget
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }

    fn spend(&mut self, cost: u64) -> Result<(), EvalError> {
        if self.budget < cost {
            return Err(EvalError::CognitiveOverload {
                budget: self.budget,
                required: cost,
            });
        }
        self.budget -= cost;
        Ok(())
    }

    pub fn eval_program(&mut self, program: &Program) -> Result<Value, EvalError> {
        // First pass: register functions, agents, types
        for decl in &program.declarations {
            match decl {
                Declaration::Function(f) => {
                    self.functions.insert(f.name.clone(), Callable::Function(f.clone()));
                }
                Declaration::Agent(a) => {
                    self.functions.insert(a.name.clone(), Callable::Agent(a.clone()));
                }
                Declaration::Type(t) => {
                    self.types.insert(t.name.clone(), t.clone());
                }
                Declaration::Statement(_) => {}
            }
        }

        // Second pass: execute top-level statements
        // Each top-level statement is a transaction boundary —
        // snapshot after completion (call stack is empty).
        let mut last = Value::Void;
        for decl in &program.declarations {
            if let Declaration::Statement(stmt) = decl {
                last = self.eval_statement(stmt)?;
                self.persist_snapshot();
            }
        }
        Ok(last)
    }

    fn eval_statement(&mut self, stmt: &Statement) -> Result<Value, EvalError> {
        match stmt {
            Statement::Let(l) => {
                self.spend(1)?;
                let value = self.eval_expr(&l.value)?;
                self.env.define(l.name.clone(), value);
                Ok(Value::Void)
            }
            Statement::Return(r) => {
                let value = match &r.value {
                    Some(expr) => self.eval_expr(expr)?,
                    None => Value::Void,
                };
                Err(EvalError::Return(value))
            }
            Statement::Expression(e) => self.eval_expr(&e.expr),
            Statement::Cb(cb) => {
                self.budget = cb.budget;
                Ok(Value::Void)
            }
        }
    }

    fn eval_block(&mut self, block: &Block) -> Result<Value, EvalError> {
        self.env.push_scope();
        let mut last = Value::Void;
        for stmt in &block.statements {
            last = self.eval_statement(stmt)?;
        }
        self.env.pop_scope();
        Ok(last)
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, EvalError> {
        match expr {
            Expr::IntLiteral(n) => Ok(Value::Int(*n)),
            Expr::FloatLiteral(n) => Ok(Value::Float(*n)),
            Expr::StringLiteral(s) => Ok(Value::String(s.clone())),
            Expr::BoolLiteral(b) => Ok(Value::Bool(*b)),
            Expr::Identifier(name) => {
                self.spend(1)?;
                self.env
                    .get(name)
                    .cloned()
                    .ok_or_else(|| EvalError::UndefinedVariable(name.clone()))
            }
            Expr::Binary(b) => self.eval_binary(b),
            Expr::Unary(u) => self.eval_unary(u),
            Expr::Call(c) => self.eval_call(c),
            Expr::If(i) => self.eval_if(i),
            Expr::FieldAccess(fa) => self.eval_field_access(fa),
            Expr::Prompt(p) => self.eval_prompt(p),
            Expr::Validate(v) => self.eval_validate(v),
            Expr::Explore(e) => self.eval_explore(e),
            Expr::ListLiteral(items) => {
                self.spend(1)?;
                let mut vals = Vec::with_capacity(items.len());
                for item in items {
                    vals.push(self.eval_expr(item)?);
                }
                Ok(Value::List(vals))
            }
            Expr::MapLiteral(entries) => {
                self.spend(1)?;
                let mut vals = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    vals.push((self.eval_expr(k)?, self.eval_expr(v)?));
                }
                Ok(Value::Map(vals))
            }
        }
    }

    fn eval_binary(&mut self, expr: &BinaryExpr) -> Result<Value, EvalError> {
        self.spend(1)?;
        let left = self.eval_expr(&expr.left)?;
        let right = self.eval_expr(&expr.right)?;

        match (&left, &expr.op, &right) {
            // Int arithmetic
            (Value::Int(a), BinaryOp::Add, Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Int(a), BinaryOp::Sub, Value::Int(b)) => Ok(Value::Int(a - b)),
            (Value::Int(a), BinaryOp::Mul, Value::Int(b)) => Ok(Value::Int(a * b)),
            (Value::Int(a), BinaryOp::Div, Value::Int(b)) => {
                if *b == 0 { return Err(EvalError::DivisionByZero); }
                Ok(Value::Int(a / b))
            }

            // Float arithmetic
            (Value::Float(a), BinaryOp::Add, Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Float(a), BinaryOp::Sub, Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Float(a), BinaryOp::Mul, Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Float(a), BinaryOp::Div, Value::Float(b)) => {
                if *b == 0.0 { return Err(EvalError::DivisionByZero); }
                Ok(Value::Float(a / b))
            }

            // Mixed int/float
            (Value::Int(a), BinaryOp::Add, Value::Float(b)) => Ok(Value::Float(*a as f64 + b)),
            (Value::Float(a), BinaryOp::Add, Value::Int(b)) => Ok(Value::Float(a + *b as f64)),
            (Value::Int(a), BinaryOp::Sub, Value::Float(b)) => Ok(Value::Float(*a as f64 - b)),
            (Value::Float(a), BinaryOp::Sub, Value::Int(b)) => Ok(Value::Float(a - *b as f64)),
            (Value::Int(a), BinaryOp::Mul, Value::Float(b)) => Ok(Value::Float(*a as f64 * b)),
            (Value::Float(a), BinaryOp::Mul, Value::Int(b)) => Ok(Value::Float(a * *b as f64)),
            (Value::Int(a), BinaryOp::Div, Value::Float(b)) => {
                if *b == 0.0 { return Err(EvalError::DivisionByZero); }
                Ok(Value::Float(*a as f64 / b))
            }
            (Value::Float(a), BinaryOp::Div, Value::Int(b)) => {
                if *b == 0 { return Err(EvalError::DivisionByZero); }
                Ok(Value::Float(a / *b as f64))
            }

            // String concat
            (Value::String(a), BinaryOp::Add, Value::String(b)) => {
                Ok(Value::String(format!("{a}{b}")))
            }

            // Int comparisons
            (Value::Int(a), BinaryOp::Eq, Value::Int(b)) => Ok(Value::Bool(a == b)),
            (Value::Int(a), BinaryOp::NotEq, Value::Int(b)) => Ok(Value::Bool(a != b)),
            (Value::Int(a), BinaryOp::Lt, Value::Int(b)) => Ok(Value::Bool(a < b)),
            (Value::Int(a), BinaryOp::Gt, Value::Int(b)) => Ok(Value::Bool(a > b)),
            (Value::Int(a), BinaryOp::LtEq, Value::Int(b)) => Ok(Value::Bool(a <= b)),
            (Value::Int(a), BinaryOp::GtEq, Value::Int(b)) => Ok(Value::Bool(a >= b)),

            // Float comparisons
            (Value::Float(a), BinaryOp::Eq, Value::Float(b)) => Ok(Value::Bool(a == b)),
            (Value::Float(a), BinaryOp::NotEq, Value::Float(b)) => Ok(Value::Bool(a != b)),
            (Value::Float(a), BinaryOp::Lt, Value::Float(b)) => Ok(Value::Bool(a < b)),
            (Value::Float(a), BinaryOp::Gt, Value::Float(b)) => Ok(Value::Bool(a > b)),
            (Value::Float(a), BinaryOp::LtEq, Value::Float(b)) => Ok(Value::Bool(a <= b)),
            (Value::Float(a), BinaryOp::GtEq, Value::Float(b)) => Ok(Value::Bool(a >= b)),

            // Mixed int/float comparisons
            (Value::Int(a), BinaryOp::Lt, Value::Float(b)) => Ok(Value::Bool((*a as f64) < *b)),
            (Value::Float(a), BinaryOp::Lt, Value::Int(b)) => Ok(Value::Bool(*a < *b as f64)),
            (Value::Int(a), BinaryOp::Gt, Value::Float(b)) => Ok(Value::Bool((*a as f64) > *b)),
            (Value::Float(a), BinaryOp::Gt, Value::Int(b)) => Ok(Value::Bool(*a > *b as f64)),
            (Value::Int(a), BinaryOp::LtEq, Value::Float(b)) => Ok(Value::Bool((*a as f64) <= *b)),
            (Value::Float(a), BinaryOp::LtEq, Value::Int(b)) => Ok(Value::Bool(*a <= *b as f64)),
            (Value::Int(a), BinaryOp::GtEq, Value::Float(b)) => Ok(Value::Bool((*a as f64) >= *b)),
            (Value::Float(a), BinaryOp::GtEq, Value::Int(b)) => Ok(Value::Bool(*a >= *b as f64)),
            (Value::Int(a), BinaryOp::Eq, Value::Float(b)) => Ok(Value::Bool((*a as f64) == *b)),
            (Value::Float(a), BinaryOp::Eq, Value::Int(b)) => Ok(Value::Bool(*a == *b as f64)),
            (Value::Int(a), BinaryOp::NotEq, Value::Float(b)) => Ok(Value::Bool((*a as f64) != *b)),
            (Value::Float(a), BinaryOp::NotEq, Value::Int(b)) => Ok(Value::Bool(*a != *b as f64)),

            // String comparisons
            (Value::String(a), BinaryOp::Eq, Value::String(b)) => Ok(Value::Bool(a == b)),
            (Value::String(a), BinaryOp::NotEq, Value::String(b)) => Ok(Value::Bool(a != b)),

            // Bool comparisons
            (Value::Bool(a), BinaryOp::Eq, Value::Bool(b)) => Ok(Value::Bool(a == b)),
            (Value::Bool(a), BinaryOp::NotEq, Value::Bool(b)) => Ok(Value::Bool(a != b)),

            _ => Err(EvalError::TypeError {
                expected: format!("compatible types for {:?}", expr.op),
                got: format!("{} and {}", left.type_name(), right.type_name()),
            }),
        }
    }

    fn eval_unary(&mut self, expr: &UnaryExpr) -> Result<Value, EvalError> {
        self.spend(1)?;
        let operand = self.eval_expr(&expr.operand)?;
        match (&expr.op, &operand) {
            (UnaryOp::Neg, Value::Int(n)) => Ok(Value::Int(-n)),
            (UnaryOp::Neg, Value::Float(n)) => Ok(Value::Float(-n)),
            (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            _ => Err(EvalError::TypeError {
                expected: format!("valid operand for {:?}", expr.op),
                got: operand.type_name().to_string(),
            }),
        }
    }

    fn eval_call(&mut self, expr: &CallExpr) -> Result<Value, EvalError> {
        self.spend(5)?;

        // Built-in functions
        match expr.callee.as_str() {
            "print" => {
                self.require_cap(CapKind::Stdout)?;
                let mut parts = Vec::new();
                for arg in &expr.args {
                    parts.push(format!("{}", self.eval_expr(arg)?));
                }
                let line = parts.join(" ");
                self.output.push(line);
                return Ok(Value::Void);
            }
            "len" => {
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch { expected: 1, got: expr.args.len() });
                }
                let val = self.eval_expr(&expr.args[0])?;
                return match val {
                    Value::String(s) => Ok(Value::Int(s.len() as i64)),
                    Value::List(items) => Ok(Value::Int(items.len() as i64)),
                    Value::Map(entries) => Ok(Value::Int(entries.len() as i64)),
                    _ => Err(EvalError::TypeError {
                        expected: "string, list, or map".into(),
                        got: val.type_name().to_string(),
                    }),
                };
            }
            "push" => {
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch { expected: 2, got: expr.args.len() });
                }
                let list = self.eval_expr(&expr.args[0])?;
                let item = self.eval_expr(&expr.args[1])?;
                return match list {
                    Value::List(mut items) => {
                        items.push(item);
                        Ok(Value::List(items))
                    }
                    _ => Err(EvalError::TypeError {
                        expected: "list".into(),
                        got: list.type_name().to_string(),
                    }),
                };
            }
            "get" => {
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch { expected: 2, got: expr.args.len() });
                }
                let collection = self.eval_expr(&expr.args[0])?;
                let key = self.eval_expr(&expr.args[1])?;
                return match (&collection, &key) {
                    (Value::List(items), Value::Int(idx)) => {
                        let i = *idx as usize;
                        items.get(i).cloned().ok_or_else(|| EvalError::General(
                            format!("index {i} out of bounds (len {})", items.len()),
                        ))
                    }
                    (Value::Map(entries), _) => {
                        for (k, v) in entries {
                            if *k == key {
                                return Ok(v.clone());
                            }
                        }
                        Err(EvalError::General(format!("key not found in map")))
                    }
                    _ => Err(EvalError::TypeError {
                        expected: "list or map".into(),
                        got: collection.type_name().to_string(),
                    }),
                };
            }
            "map_of" => {
                if expr.args.len() % 2 != 0 {
                    return Err(EvalError::General(
                        "map_of requires an even number of arguments (key, value pairs)".into(),
                    ));
                }
                let mut entries = Vec::new();
                let mut i = 0;
                while i < expr.args.len() {
                    let k = self.eval_expr(&expr.args[i])?;
                    let v = self.eval_expr(&expr.args[i + 1])?;
                    entries.push((k, v));
                    i += 2;
                }
                return Ok(Value::Map(entries));
            }
            "typeof" => {
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch { expected: 1, got: expr.args.len() });
                }
                let val = self.eval_expr(&expr.args[0])?;
                return Ok(Value::String(val.type_name().to_string()));
            }
            _ => {}
        }

        // User-defined functions/agents
        let callable = self.functions.get(&expr.callee).cloned()
            .ok_or_else(|| EvalError::UndefinedFunction(expr.callee.clone()))?;

        let (params, body, is_agent) = match &callable {
            Callable::Function(f) => (&f.params, &f.body, false),
            Callable::Agent(a) => (&a.params, &a.body, true),
        };

        if params.len() != expr.args.len() {
            return Err(EvalError::ArityMismatch {
                expected: params.len(),
                got: expr.args.len(),
            });
        }

        // Evaluate arguments
        let mut arg_values = Vec::new();
        for arg in &expr.args {
            arg_values.push(self.eval_expr(arg)?);
        }

        // Save state for agents (isolated scope)
        let saved_env = if is_agent { Some(self.env.clone()) } else { None };

        // Set up call scope
        self.env.push_scope();
        for (param, value) in params.iter().zip(arg_values) {
            self.env.define(param.name.clone(), value);
        }

        // Execute body
        let result = match self.eval_block_inner(&body.statements) {
            Ok(v) => v,
            Err(EvalError::Return(v)) => v,
            Err(e) => {
                self.env.pop_scope();
                if let Some(env) = saved_env {
                    self.env = env;
                }
                return Err(e);
            }
        };

        self.env.pop_scope();

        // Restore env for agents
        if let Some(env) = saved_env {
            self.env = env;
        }

        Ok(result)
    }

    fn eval_block_inner(&mut self, statements: &[Statement]) -> Result<Value, EvalError> {
        let mut last = Value::Void;
        for stmt in statements {
            last = self.eval_statement(stmt)?;
        }
        Ok(last)
    }

    fn eval_if(&mut self, expr: &IfExpr) -> Result<Value, EvalError> {
        self.spend(1)?;
        let condition = self.eval_expr(&expr.condition)?;
        if condition.is_truthy() {
            self.eval_block(&expr.then_block)
        } else if let Some(else_block) = &expr.else_block {
            self.eval_block(else_block)
        } else {
            Ok(Value::Void)
        }
    }

    fn eval_field_access(&mut self, expr: &FieldAccessExpr) -> Result<Value, EvalError> {
        self.spend(1)?;
        let object = self.eval_expr(&expr.object)?;
        match &object {
            Value::Struct(type_name, fields) => {
                fields.get(&expr.field).cloned().ok_or_else(|| EvalError::UndefinedField {
                    type_name: type_name.clone(),
                    field: expr.field.clone(),
                })
            }
            _ => Err(EvalError::NotAStruct(object.type_name().to_string())),
        }
    }

    // --- AI-native constructs ---

    fn eval_prompt(&mut self, expr: &PromptExpr) -> Result<Value, EvalError> {
        self.require_cap(CapKind::Prompt)?;
        self.spend(50)?;
        let input = self.eval_expr(&expr.input)?;
        let input_str = format!("{input}");

        // Collect type field info for user-defined types
        let type_fields = self.collect_type_fields(&expr.return_type);
        let fields_ref: Vec<(&str, &str)> = type_fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let fields_opt = if fields_ref.is_empty() {
            None
        } else {
            Some(fields_ref.as_slice())
        };

        if let Some(backend) = self.llm_backend {
            backend
                .complete(&expr.instruction, &input_str, &expr.return_type, fields_opt)
                .map_err(|e| EvalError::General(format!("{e}")))
        } else {
            // Fallback: built-in mock
            crate::llm::MockBackend::new()
                .complete(&expr.instruction, &input_str, &expr.return_type, fields_opt)
                .map_err(|e| EvalError::General(format!("{e}")))
        }
    }

    fn collect_type_fields(&self, type_ann: &TypeAnnotation) -> Vec<(String, String)> {
        if let TypeAnnotation::Named(name) = type_ann {
            if let Some(type_decl) = self.types.get(name) {
                return type_decl
                    .fields
                    .iter()
                    .map(|f| {
                        let type_name = match &f.type_annotation {
                            TypeAnnotation::Named(n) => n.clone(),
                            TypeAnnotation::Generic(n, _) => n.clone(),
                        };
                        (f.name.clone(), type_name)
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    fn eval_validate(&mut self, expr: &ValidateExpr) -> Result<Value, EvalError> {
        let target = self.eval_expr(&expr.target)?;

        for (i, predicate) in expr.predicates.iter().enumerate() {
            self.spend(1)?;
            let result = self.eval_expr(predicate)?;
            match result {
                Value::Bool(true) => {}
                Value::Bool(false) => {
                    return Err(EvalError::ValidationFailed {
                        predicate_index: i,
                        detail: format!("predicate #{i} evaluated to false"),
                    });
                }
                _ => {
                    return Err(EvalError::TypeError {
                        expected: "bool".into(),
                        got: result.type_name().to_string(),
                    });
                }
            }
        }

        Ok(target)
    }

    fn eval_explore(&mut self, expr: &ExploreBlock) -> Result<Value, EvalError> {
        self.require_cap(CapKind::VcsWrite)?;
        self.spend(1)?;

        // Save current state
        let saved_env = self.env.clone();
        let saved_budget = self.budget;

        // Run in isolated context
        let result = self.eval_block(&expr.body);

        match result {
            Ok(value) => {
                // Success: create a VCS branch if store/refs are available
                if let Some((store, refs)) = &self.vcs {
                    let _ = refs.create_branch(&expr.name, None);
                    let _ = store;
                    self.explore_branches.push(expr.name.clone());
                }
                // Transaction boundary: explore block completed, call stack empty
                self.persist_snapshot();
                Ok(value)
            }
            Err(e) => {
                // Failure: restore everything, no side effects
                self.env = saved_env;
                self.budget = saved_budget;
                Err(e)
            }
        }
    }

    pub fn explore_branches(&self) -> &[String] {
        &self.explore_branches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn eval(source: &str) -> Result<Value, EvalError> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program)
    }

    fn eval_with_budget(source: &str, budget: u64) -> Result<Value, EvalError> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(budget);
        evaluator.grant_all();
        evaluator.eval_program(&program)
    }

    fn eval_output(source: &str) -> Vec<String> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        evaluator.output().to_vec()
    }

    // --- Literals ---

    #[test]
    fn int_literal() {
        assert_eq!(eval("42;"), Ok(Value::Int(42)));
    }

    #[test]
    fn float_literal() {
        assert_eq!(eval("3.14;"), Ok(Value::Float(3.14)));
    }

    #[test]
    fn string_literal() {
        assert_eq!(eval(r#""hello";"#), Ok(Value::String("hello".into())));
    }

    #[test]
    fn bool_literal() {
        assert_eq!(eval("true;"), Ok(Value::Bool(true)));
        assert_eq!(eval("false;"), Ok(Value::Bool(false)));
    }

    // --- Arithmetic ---

    #[test]
    fn int_arithmetic() {
        assert_eq!(eval("2 + 3;"), Ok(Value::Int(5)));
        assert_eq!(eval("10 - 4;"), Ok(Value::Int(6)));
        assert_eq!(eval("3 * 7;"), Ok(Value::Int(21)));
        assert_eq!(eval("15 / 3;"), Ok(Value::Int(5)));
    }

    #[test]
    fn float_arithmetic() {
        assert_eq!(eval("1.5 + 2.5;"), Ok(Value::Float(4.0)));
    }

    #[test]
    fn mixed_arithmetic() {
        assert_eq!(eval("1 + 2.5;"), Ok(Value::Float(3.5)));
        assert_eq!(eval("2.5 + 1;"), Ok(Value::Float(3.5)));
    }

    #[test]
    fn string_concat() {
        assert_eq!(eval(r#""hello" + " " + "world";"#), Ok(Value::String("hello world".into())));
    }

    #[test]
    fn division_by_zero() {
        assert!(matches!(eval("1 / 0;"), Err(EvalError::DivisionByZero)));
    }

    #[test]
    fn precedence() {
        assert_eq!(eval("2 + 3 * 4;"), Ok(Value::Int(14)));
        assert_eq!(eval("(2 + 3) * 4;"), Ok(Value::Int(20)));
    }

    // --- Unary ---

    #[test]
    fn unary_neg() {
        assert_eq!(eval("-5;"), Ok(Value::Int(-5)));
        assert_eq!(eval("-3.14;"), Ok(Value::Float(-3.14)));
    }

    #[test]
    fn unary_not() {
        assert_eq!(eval("!true;"), Ok(Value::Bool(false)));
        assert_eq!(eval("!false;"), Ok(Value::Bool(true)));
    }

    // --- Comparisons ---

    #[test]
    fn comparisons() {
        assert_eq!(eval("1 < 2;"), Ok(Value::Bool(true)));
        assert_eq!(eval("2 > 1;"), Ok(Value::Bool(true)));
        assert_eq!(eval("1 == 1;"), Ok(Value::Bool(true)));
        assert_eq!(eval("1 != 2;"), Ok(Value::Bool(true)));
        assert_eq!(eval("1 <= 1;"), Ok(Value::Bool(true)));
        assert_eq!(eval("1 >= 1;"), Ok(Value::Bool(true)));
    }

    #[test]
    fn string_equality() {
        assert_eq!(eval(r#""a" == "a";"#), Ok(Value::Bool(true)));
        assert_eq!(eval(r#""a" != "b";"#), Ok(Value::Bool(true)));
    }

    // --- Variables ---

    #[test]
    fn let_and_use() {
        assert_eq!(eval("let x = 42; x;"), Ok(Value::Int(42)));
    }

    #[test]
    fn let_with_expression() {
        assert_eq!(eval("let x = 2 + 3; x * 2;"), Ok(Value::Int(10)));
    }

    #[test]
    fn undefined_variable() {
        assert!(matches!(eval("x;"), Err(EvalError::UndefinedVariable(_))));
    }

    // --- If ---

    #[test]
    fn if_true() {
        let output = eval_output(r#"
            if true {
                print("yes");
            }
        "#);
        assert_eq!(output, vec!["yes"]);
    }

    #[test]
    fn if_false_no_else() {
        let output = eval_output(r#"
            if false {
                print("yes");
            }
        "#);
        assert!(output.is_empty());
    }

    #[test]
    fn if_else() {
        let output = eval_output(r#"
            if false {
                print("yes");
            } else {
                print("no");
            }
        "#);
        assert_eq!(output, vec!["no"]);
    }

    // --- Functions ---

    #[test]
    fn function_call() {
        assert_eq!(eval(r#"
            fn add(a: int, b: int) -> int {
                return a + b;
            }
            add(2, 3);
        "#), Ok(Value::Int(5)));
    }

    #[test]
    fn recursive_function() {
        assert_eq!(eval(r#"
            fn factorial(n: int) -> int {
                if n <= 1 {
                    return 1;
                }
                return n * factorial(n - 1);
            }
            factorial(5);
        "#), Ok(Value::Int(120)));
    }

    #[test]
    fn function_arity_mismatch() {
        assert!(matches!(eval(r#"
            fn f(x: int) -> int { return x; }
            f(1, 2);
        "#), Err(EvalError::ArityMismatch { .. })));
    }

    #[test]
    fn undefined_function() {
        assert!(matches!(eval("foo();"), Err(EvalError::UndefinedFunction(_))));
    }

    // --- Built-ins ---

    #[test]
    fn print_builtin() {
        let output = eval_output(r#"print("hello", 42);"#);
        assert_eq!(output, vec!["hello 42"]);
    }

    #[test]
    fn len_builtin() {
        assert_eq!(eval(r#"len("hello");"#), Ok(Value::Int(5)));
    }

    #[test]
    fn typeof_builtin() {
        assert_eq!(eval(r#"typeof(42);"#), Ok(Value::String("int".into())));
        assert_eq!(eval(r#"typeof("hi");"#), Ok(Value::String("string".into())));
    }

    // --- Cognitive Budget ---

    #[test]
    fn cb_exhaustion() {
        let result = eval_with_budget("let a = 1; let b = 2; let c = 3; let d = 4; let e = 5;", 3);
        assert!(matches!(result, Err(EvalError::CognitiveOverload { .. })));
    }

    #[test]
    fn cb_function_call_cost() {
        // Function call costs 5 CB
        let result = eval_with_budget(r#"
            fn f() -> int { return 1; }
            f();
        "#, 4);
        assert!(matches!(result, Err(EvalError::CognitiveOverload { .. })));
    }

    #[test]
    fn cb_statement_override() {
        assert_eq!(eval(r#"
            fn f() -> int {
                cb 100;
                return 42;
            }
            f();
        "#), Ok(Value::Int(42)));
    }

    #[test]
    fn cb_recursive_exhaustion() {
        let result = eval_with_budget(r#"
            fn loop_forever(n: int) -> int {
                return loop_forever(n + 1);
            }
            loop_forever(0);
        "#, 100);
        assert!(matches!(result, Err(EvalError::CognitiveOverload { .. })));
    }

    // --- Agent ---

    #[test]
    fn agent_basic() {
        assert_eq!(eval(r#"
            agent greet(name: string) -> string {
                return "hello " + name;
            }
            greet("world");
        "#), Ok(Value::String("hello world".into())));
    }

    #[test]
    fn agent_isolation() {
        // Agent should not see outer mutable state
        let output = eval_output(r#"
            let x = 10;
            agent f(n: int) -> int {
                return n + 1;
            }
            let result = f(5);
            print(result);
            print(x);
        "#);
        assert_eq!(output, vec!["6", "10"]);
    }

    // --- Prompt (mock) ---

    #[test]
    fn prompt_mock_primitive() {
        assert_eq!(eval(r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#), Ok(Value::Int(0)));
    }

    #[test]
    fn prompt_mock_struct() {
        let result = eval(r#"
            type Report {
                summary: string,
                score: float
            }
            let data = "test";
            prompt("analyze", data) -> Report;
        "#);
        match result {
            Ok(Value::Struct(name, fields)) => {
                assert_eq!(name, "Report");
                assert_eq!(fields.get("summary"), Some(&Value::String("mock".into())));
                assert_eq!(fields.get("score"), Some(&Value::Float(0.0)));
            }
            _ => panic!("expected struct, got {result:?}"),
        }
    }

    #[test]
    fn prompt_costs_50_cb() {
        let result = eval_with_budget(r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#, 49);
        assert!(matches!(result, Err(EvalError::CognitiveOverload { .. })));
    }

    // --- Validate ---

    #[test]
    fn validate_passes() {
        assert_eq!(eval(r#"
            let x = 10;
            validate x { x > 5 };
        "#), Ok(Value::Int(10)));
    }

    #[test]
    fn validate_fails() {
        let result = eval(r#"
            let x = 3;
            validate x { x > 5 };
        "#);
        assert!(matches!(result, Err(EvalError::ValidationFailed { .. })));
    }

    #[test]
    fn validate_multiple_predicates() {
        assert_eq!(eval(r#"
            let x = 10;
            validate x { x > 5, x < 20 };
        "#), Ok(Value::Int(10)));
    }

    #[test]
    fn validate_second_predicate_fails() {
        let result = eval(r#"
            let x = 10;
            validate x { x > 5, x < 8 };
        "#);
        match result {
            Err(EvalError::ValidationFailed { predicate_index, .. }) => {
                assert_eq!(predicate_index, 1);
            }
            _ => panic!("expected validation failure on predicate 1"),
        }
    }

    // --- Explore ---

    #[test]
    fn explore_success() {
        let result = eval(r#"
            fn f() {
                explore "test" {
                    let x = 42;
                };
            }
            f();
        "#);
        assert!(result.is_ok());
    }

    #[test]
    fn explore_failure_restores_state() {
        // Explore that fails should not affect outer state
        let result = eval(r#"
            let x = 10;
            explore "failing" {
                let y = 1 / 0;
            }
        "#);
        // The explore block fails, which propagates as an error
        // but the outer state should be restored
        assert!(result.is_err());
    }

    // --- Field access ---

    #[test]
    fn field_access_on_struct() {
        let result = eval(r#"
            type Report {
                summary: string,
                score: float
            }
            let data = "test";
            let r = prompt("analyze", data) -> Report;
            r.summary;
        "#);
        assert_eq!(result, Ok(Value::String("mock".into())));
    }

    #[test]
    fn field_access_on_non_struct() {
        let result = eval(r#"
            let x = 42;
            x.field;
        "#);
        assert!(matches!(result, Err(EvalError::NotAStruct(_))));
    }

    // --- Full program ---

    #[test]
    fn full_agent_program() {
        let source = r#"
            type Category {
                label: string,
                confidence: float
            }

            fn check_result(cat: Category) -> bool {
                return cat.confidence > 0.5;
            }

            agent classifier(text: string) -> Category {
                cb 1000;
                let result = prompt("Classify this text", text) -> Category;
                validate result {
                    result.label != "unknown"
                }
                return result;
            }

            let input = "Hello, world!";
            let result = classifier(input);
            print(result.label);
        "#;
        let output = eval_output(source);
        assert_eq!(output, vec!["mock"]);
    }

    // --- Budget tracking ---

    #[test]
    fn budget_tracking() {
        let program = Parser::parse_source("let x = 1 + 2;").unwrap();
        let mut evaluator = Evaluator::new(100);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert!(evaluator.budget_remaining() < 100);
    }

    // --- Capability-Based Security (OCap) ---

    #[test]
    fn prompt_requires_capability() {
        let program = Parser::parse_source(r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn prompt_with_capability_succeeds() {
        let program = Parser::parse_source(r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant(CapKind::Prompt);
        let result = evaluator.eval_program(&program);
        assert_eq!(result, Ok(Value::Int(0)));
    }

    #[test]
    fn print_requires_capability() {
        let program = Parser::parse_source(r#"print(42);"#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn print_with_capability_succeeds() {
        let program = Parser::parse_source(r#"print(42);"#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant(CapKind::Stdout);
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["42"]);
    }

    #[test]
    fn explore_requires_vcs_write_capability() {
        let program = Parser::parse_source(r#"
            explore "test-branch" {
                let x = 42;
            }
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn explore_with_capability_succeeds() {
        let program = Parser::parse_source(r#"
            explore "test-branch" {
                let x = 42;
            }
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant(CapKind::VcsWrite);
        let result = evaluator.eval_program(&program);
        assert!(result.is_ok());
    }

    #[test]
    fn no_caps_pure_code_works() {
        // Arithmetic, let, if, functions should work without any capabilities
        let program = Parser::parse_source(r#"
            fn add(a: int, b: int) -> int { return a + b; }
            let x = add(2, 3);
            if x > 4 { x; } else { 0; };
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert_eq!(result, Ok(Value::Int(5)));
    }

    #[test]
    fn grant_all_allows_everything() {
        let program = Parser::parse_source(r#"
            let x = "input";
            let r = prompt("classify", x) -> int;
            print(r);
        "#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["0"]);
    }

    #[test]
    fn revoke_blocks_subsequent_ops() {
        let program = Parser::parse_source(r#"print(42);"#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant(CapKind::Stdout);
        evaluator.revoke(CapKind::Stdout);
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn capability_denied_error_message() {
        let program = Parser::parse_source(r#"print(42);"#).unwrap();
        let mut evaluator = Evaluator::new(10000);
        let result = evaluator.eval_program(&program);
        match result {
            Err(EvalError::CapabilityDenied(e)) => {
                let msg = format!("{e}");
                assert!(msg.contains("stdout"), "error should mention the capability kind");
            }
            other => panic!("expected CapabilityDenied, got {other:?}"),
        }
    }

    // --- Orthogonal Persistence ---

    #[test]
    fn capture_and_restore_snapshot() {
        let program = Parser::parse_source("let x = 42; let y = 10;").unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let snap = evaluator.capture_snapshot();
        assert_eq!(snap.budget_remaining, evaluator.budget_remaining());

        // Create a fresh evaluator and restore
        let mut evaluator2 = Evaluator::new(0);
        evaluator2.grant_all();
        evaluator2.restore_snapshot(&snap);
        assert_eq!(evaluator2.budget_remaining(), snap.budget_remaining);
    }

    #[test]
    fn persistence_creates_snapshots_per_statement() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let x = 1; let y = 2; let z = 3;").unwrap();
        let mut evaluator = Evaluator::new(10000)
            .with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        // 3 top-level statements → 3 snapshots
        assert_eq!(evaluator.snapshot_history().len(), 3);
    }

    #[test]
    fn persistence_snapshots_are_loadable() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let x = 42;").unwrap();
        let mut evaluator = Evaluator::new(10000)
            .with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let hash = &evaluator.snapshot_history()[0];
        let mgr = crate::snapshot::SnapshotManager::new(&store);
        let snap = mgr.load(hash).unwrap();
        assert!(snap.budget_remaining < 10000);
        assert_eq!(snap.scopes.len(), 1); // global scope
    }

    #[test]
    fn persistence_rollback_restores_earlier_state() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let x = 1; let y = 2;").unwrap();
        let mut evaluator = Evaluator::new(10000)
            .with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let history = evaluator.snapshot_history().to_vec();
        assert_eq!(history.len(), 2);

        // Load first snapshot (after "let x = 1;")
        let mgr = crate::snapshot::SnapshotManager::new(&store);
        let snap1 = mgr.load(&history[0]).unwrap();

        // First snapshot should have x=1 but not y
        let scope = &snap1.scopes[0];
        assert_eq!(scope.get("x"), Some(&Value::Int(1)));
        assert_eq!(scope.get("y"), None);

        // Second snapshot should have both
        let snap2 = mgr.load(&history[1]).unwrap();
        let scope2 = &snap2.scopes[0];
        assert_eq!(scope2.get("x"), Some(&Value::Int(1)));
        assert_eq!(scope2.get("y"), Some(&Value::Int(2)));
    }

    #[test]
    fn no_persistence_without_store() {
        let program = Parser::parse_source("let x = 1;").unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        // No persistence configured → no snapshots
        assert!(evaluator.snapshot_history().is_empty());
    }

    #[test]
    fn persistence_with_output() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source(r#"print(42); print(99);"#).unwrap();
        let mut evaluator = Evaluator::new(10000)
            .with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let history = evaluator.snapshot_history().to_vec();
        let mgr = crate::snapshot::SnapshotManager::new(&store);

        // After first print: output has ["42"]
        let snap1 = mgr.load(&history[0]).unwrap();
        assert_eq!(snap1.output, vec!["42"]);

        // After second print: output has ["42", "99"]
        let snap2 = mgr.load(&history[1]).unwrap();
        assert_eq!(snap2.output, vec!["42", "99"]);
    }

    #[test]
    fn snapshot_content_addressed_dedup() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        // Two identical programs should produce identical snapshot hashes
        // for equivalent states
        let program = Parser::parse_source("let x = 42;").unwrap();

        let mut eval1 = Evaluator::new(10000).with_persistence(&store);
        eval1.grant_all();
        eval1.eval_program(&program).unwrap();

        let mut eval2 = Evaluator::new(10000).with_persistence(&store);
        eval2.grant_all();
        eval2.eval_program(&program).unwrap();

        assert_eq!(
            eval1.snapshot_history()[0],
            eval2.snapshot_history()[0],
            "identical state should produce identical snapshot hash"
        );
    }

    // --- Collections ---

    #[test]
    fn list_literal_empty() {
        assert_eq!(eval("[];"), Ok(Value::List(vec![])));
    }

    #[test]
    fn list_literal_items() {
        assert_eq!(
            eval("[1, 2, 3];"),
            Ok(Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]))
        );
    }

    #[test]
    fn list_len() {
        assert_eq!(eval("len([1, 2, 3]);"), Ok(Value::Int(3)));
    }

    #[test]
    fn list_push() {
        assert_eq!(
            eval("push([1, 2], 3);"),
            Ok(Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]))
        );
    }

    #[test]
    fn list_get() {
        assert_eq!(eval("get([10, 20, 30], 1);"), Ok(Value::Int(20)));
    }

    #[test]
    fn list_get_out_of_bounds() {
        assert!(matches!(eval("get([1], 5);"), Err(EvalError::General(_))));
    }

    #[test]
    fn list_is_truthy() {
        let output = eval_output("if [1] { print(\"yes\"); } if [] { print(\"no\"); }");
        assert_eq!(output, vec!["yes"]);
    }

    #[test]
    fn list_display() {
        let output = eval_output("print([1, 2, 3]);");
        assert_eq!(output, vec!["[1, 2, 3]"]);
    }

    #[test]
    fn map_of_builtin() {
        assert_eq!(
            eval("map_of(\"a\", 1, \"b\", 2);"),
            Ok(Value::Map(vec![
                (Value::String("a".into()), Value::Int(1)),
                (Value::String("b".into()), Value::Int(2)),
            ]))
        );
    }

    #[test]
    fn map_of_odd_args_error() {
        assert!(matches!(eval("map_of(\"a\", 1, \"b\");"), Err(EvalError::General(_))));
    }

    #[test]
    fn map_len() {
        assert_eq!(eval("len(map_of(\"x\", 1, \"y\", 2));"), Ok(Value::Int(2)));
    }

    #[test]
    fn map_get() {
        assert_eq!(eval("get(map_of(\"a\", 42), \"a\");"), Ok(Value::Int(42)));
    }

    #[test]
    fn map_get_missing_key() {
        assert!(matches!(eval("get(map_of(\"a\", 1), \"z\");"), Err(EvalError::General(_))));
    }

    #[test]
    fn list_in_variable() {
        let output = eval_output("let xs = [1, 2]; let ys = push(xs, 3); print(len(ys));");
        assert_eq!(output, vec!["3"]);
    }

    #[test]
    fn snapshot_list_round_trip() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let xs = [1, 2, 3];").unwrap();
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let history = evaluator.snapshot_history().to_vec();
        assert!(!history.is_empty());

        let mgr = crate::snapshot::SnapshotManager::new(&store);
        let snap = mgr.load(&history[0]).unwrap();
        assert!(snap.scopes.iter().any(|scope| {
            scope.get("xs") == Some(&Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]))
        }));
    }

    #[test]
    fn snapshot_map_round_trip() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let m = map_of(\"a\", 1);").unwrap();
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let history = evaluator.snapshot_history().to_vec();
        let mgr = crate::snapshot::SnapshotManager::new(&store);
        let snap = mgr.load(&history[0]).unwrap();
        assert!(snap.scopes.iter().any(|scope| {
            scope.get("m") == Some(&Value::Map(vec![
                (Value::String("a".into()), Value::Int(1)),
            ]))
        }));
    }
}
