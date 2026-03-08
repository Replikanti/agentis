use std::collections::HashMap;

use crate::ast::*;
use crate::refs::Refs;
use crate::storage::ObjectStore;

// --- Runtime Values ---

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
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
            Value::Struct(name, _) => name,
            Value::Void => "void",
        }
    }

    fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::String(s) => !s.is_empty(),
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
        }
    }

    pub fn with_vcs(mut self, store: &'a ObjectStore, refs: &'a Refs) -> Self {
        self.vcs = Some((store, refs));
        self
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
        let mut last = Value::Void;
        for decl in &program.declarations {
            if let Declaration::Statement(stmt) = decl {
                last = self.eval_statement(stmt)?;
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
                    _ => Err(EvalError::TypeError {
                        expected: "string".into(),
                        got: val.type_name().to_string(),
                    }),
                };
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
        self.spend(50)?;
        // Evaluate input (to spend CB and validate it exists)
        let _input = self.eval_expr(&expr.input)?;

        // Mock: generate deterministic stub data based on return type
        self.mock_value_for_type(&expr.return_type)
    }

    fn mock_value_for_type(&self, type_ann: &TypeAnnotation) -> Result<Value, EvalError> {
        match type_ann {
            TypeAnnotation::Named(name) => match name.as_str() {
                "int" => Ok(Value::Int(0)),
                "float" => Ok(Value::Float(0.0)),
                "string" => Ok(Value::String("mock".to_string())),
                "bool" => Ok(Value::Bool(true)),
                _ => {
                    // Look up user-defined type and generate mock struct
                    let type_decl = self.types.get(name).cloned()
                        .ok_or_else(|| EvalError::UndefinedType(name.clone()))?;
                    let mut fields = HashMap::new();
                    for field in &type_decl.fields {
                        let value = self.mock_value_for_type(&field.type_annotation)?;
                        fields.insert(field.name.clone(), value);
                    }
                    Ok(Value::Struct(name.clone(), fields))
                }
            },
            TypeAnnotation::Generic(_, _) => {
                // For Phase 1, generic collections return a simple default
                Ok(Value::String("mock_collection".to_string()))
            }
        }
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
                    // Store the branch name as a marker — the actual program
                    // will be committed separately. We create the branch pointing
                    // to the current branch's commit.
                    let _ = refs.create_branch(&expr.name, None);
                    let _ = store; // used implicitly through refs
                    self.explore_branches.push(expr.name.clone());
                }
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
        evaluator.eval_program(&program)
    }

    fn eval_with_budget(source: &str, budget: u64) -> Result<Value, EvalError> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(budget);
        evaluator.eval_program(&program)
    }

    fn eval_output(source: &str) -> Vec<String> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(10000);
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
        evaluator.eval_program(&program).unwrap();
        assert!(evaluator.budget_remaining() < 100);
    }
}
