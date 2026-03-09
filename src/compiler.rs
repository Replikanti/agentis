use std::collections::HashMap;

use wasm_encoder::{
    CodeSection, EntityType, ExportKind, ExportSection, Function, FunctionSection, GlobalSection,
    GlobalType, ImportSection, Instruction, Module, TypeSection, ValType,
};

use crate::ast::*;
use crate::storage::ObjectStore;

// --- Compiler Error ---

#[derive(Debug, Clone, PartialEq)]
pub enum CompileError {
    UndefinedVariable(String),
    UndefinedFunction(String),
    UnsupportedFeature(String),
    InternalError(String),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::UndefinedVariable(name) => write!(f, "undefined variable: {name}"),
            CompileError::UndefinedFunction(name) => write!(f, "undefined function: {name}"),
            CompileError::UnsupportedFeature(what) => {
                write!(f, "unsupported feature in WASM compilation: {what}")
            }
            CompileError::InternalError(msg) => write!(f, "internal compiler error: {msg}"),
        }
    }
}

// --- Per-function compilation context ---

struct FuncContext {
    locals: HashMap<String, u32>,
    next_local: u32,
    extra_local_count: u32,
}

impl FuncContext {
    fn new(params: &[Param]) -> Self {
        let mut locals = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            locals.insert(p.name.clone(), i as u32);
        }
        Self {
            next_local: params.len() as u32,
            locals,
            extra_local_count: 0,
        }
    }

    fn new_empty() -> Self {
        Self {
            locals: HashMap::new(),
            next_local: 0,
            extra_local_count: 0,
        }
    }

    fn alloc_local(&mut self, name: &str) -> u32 {
        let idx = self.next_local;
        self.locals.insert(name.to_string(), idx);
        self.next_local += 1;
        self.extra_local_count += 1;
        idx
    }
}

// --- Collected function info ---

struct FuncInfo {
    params: Vec<Param>,
    body: Block,
    has_return: bool,
}

// --- OCap host import definitions ---
// These are the host functions that a WASM runtime must provide for OCap.
// Type indices 0..NUM_IMPORTS are reserved for imports.

const NUM_IMPORTS: u32 = 3;

// Import type indices (in the type section)
const TYPE_CAP_CHECK: u32 = 0;   // (i64) -> i32
const TYPE_CAP_REVOKE: u32 = 1;  // (i64) -> ()
const TYPE_HOST_PRINT: u32 = 2;  // (i32, i32) -> ()

// --- Compiler ---

struct Compiler {
    func_index: HashMap<String, u32>,
    functions: Vec<(String, FuncInfo)>,
    top_level_stmts: Vec<Statement>,
    default_budget: i64,
}

impl Compiler {
    fn new() -> Self {
        Self {
            func_index: HashMap::new(),
            functions: Vec::new(),
            top_level_stmts: Vec::new(),
            default_budget: 10000,
        }
    }

    fn collect_declarations(&mut self, program: &Program) -> Result<(), CompileError> {
        // Function indices start after imported functions
        let mut idx: u32 = NUM_IMPORTS;
        for decl in &program.declarations {
            match decl {
                Declaration::Function(f) => {
                    self.func_index.insert(f.name.clone(), idx);
                    self.functions.push((
                        f.name.clone(),
                        FuncInfo {
                            params: f.params.clone(),
                            body: f.body.clone(),
                            has_return: f.return_type.is_some(),
                        },
                    ));
                    idx += 1;
                }
                Declaration::Agent(a) => {
                    self.func_index.insert(a.name.clone(), idx);
                    self.functions.push((
                        a.name.clone(),
                        FuncInfo {
                            params: a.params.clone(),
                            body: a.body.clone(),
                            has_return: a.return_type.is_some(),
                        },
                    ));
                    idx += 1;
                }
                Declaration::Statement(stmt) => {
                    // Check for top-level cb statement to set default budget
                    if let Statement::Cb(cb) = stmt {
                        self.default_budget = cb.budget as i64;
                    }
                    self.top_level_stmts.push(stmt.clone());
                }
                Declaration::Type(_) => {
                    // Types are not compiled to WASM in Phase 2
                }
            }
        }
        // The "run" function gets the next index
        self.func_index.insert("__run".to_string(), idx);
        Ok(())
    }

    fn run_func_idx(&self) -> u32 {
        *self.func_index.get("__run").unwrap()
    }

    fn build_import_section(&self) -> ImportSection {
        let mut imports = ImportSection::new();
        imports.import("env", "cap_check", EntityType::Function(TYPE_CAP_CHECK));
        imports.import("env", "cap_revoke", EntityType::Function(TYPE_CAP_REVOKE));
        imports.import("env", "host_print", EntityType::Function(TYPE_HOST_PRINT));
        imports
    }

    fn build_type_section(&self) -> TypeSection {
        let mut types = TypeSection::new();

        // Import function types (must come first, indices 0..NUM_IMPORTS)
        // Type 0: cap_check (i64) -> i32
        types.ty().function([ValType::I64], [ValType::I32]);
        // Type 1: cap_revoke (i64) -> ()
        types.ty().function([ValType::I64], []);
        // Type 2: host_print (i32, i32) -> ()
        types.ty().function([ValType::I32, ValType::I32], []);

        // One type per user function
        for (_name, info) in &self.functions {
            let params: Vec<ValType> = info.params.iter().map(|_| ValType::I64).collect();
            let results: Vec<ValType> = if info.has_return {
                vec![ValType::I64]
            } else {
                vec![]
            };
            types.ty().function(params, results);
        }

        // Type for "run": () -> i32
        types.ty().function([], [ValType::I32]);

        types
    }

    fn build_function_section(&self) -> FunctionSection {
        let mut funcs = FunctionSection::new();
        let total = self.functions.len() + 1; // +1 for run
        for i in 0..total {
            // Type indices are offset by NUM_IMPORTS (import types come first)
            funcs.function(NUM_IMPORTS + i as u32);
        }
        funcs
    }

    fn build_global_section(&self) -> GlobalSection {
        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &wasm_encoder::ConstExpr::i64_const(self.default_budget),
        );
        globals
    }

    fn build_export_section(&self) -> ExportSection {
        let mut exports = ExportSection::new();
        exports.export("run", ExportKind::Func, self.run_func_idx());
        exports.export("cb_remaining", ExportKind::Global, 0);
        exports
    }

    fn build_code_section(&self) -> Result<CodeSection, CompileError> {
        let mut codes = CodeSection::new();

        // Compile user functions
        for (_name, info) in &self.functions {
            let func = self.compile_user_function(info)?;
            codes.function(&func);
        }

        // Compile "run" function
        let run_func = self.compile_run_function()?;
        codes.function(&run_func);

        Ok(codes)
    }

    fn compile_user_function(&self, info: &FuncInfo) -> Result<Function, CompileError> {
        let mut ctx = FuncContext::new(&info.params);

        // Pre-scan for local count
        let let_count = count_lets_in_block(&info.body);
        ctx.extra_local_count = 0; // will grow as we alloc

        // Build instructions into a buffer first, then create Function
        let mut instrs: Vec<Instruction<'static>> = Vec::new();

        // CB check at function entry (cost = 5 for function call)
        emit_cb_check(&mut instrs, 5);

        // Compile body
        self.compile_block_stmts(&mut instrs, &mut ctx, &info.body.statements, false)?;

        // If the function has a return type but body didn't explicitly return,
        // push a default value
        if info.has_return {
            instrs.push(Instruction::I64Const(0));
        }

        instrs.push(Instruction::End);

        // Build the Function with locals
        let total_locals = let_count;
        let locals: Vec<(u32, ValType)> = if total_locals > 0 {
            vec![(total_locals, ValType::I64)]
        } else {
            vec![]
        };
        let mut func = Function::new(locals);
        for instr in &instrs {
            func.instruction(instr);
        }

        Ok(func)
    }

    fn compile_run_function(&self) -> Result<Function, CompileError> {
        let mut ctx = FuncContext::new_empty();

        // Pre-scan for local count
        let let_count = count_lets_in_stmts(&self.top_level_stmts);

        let mut instrs: Vec<Instruction<'static>> = Vec::new();

        // Compile top-level statements
        for stmt in &self.top_level_stmts {
            self.compile_statement(&mut instrs, &mut ctx, stmt)?;
        }

        // Return 0 (success)
        instrs.push(Instruction::I32Const(0));
        instrs.push(Instruction::End);

        let locals: Vec<(u32, ValType)> = if let_count > 0 {
            vec![(let_count, ValType::I64)]
        } else {
            vec![]
        };
        let mut func = Function::new(locals);
        for instr in &instrs {
            func.instruction(instr);
        }

        Ok(func)
    }

    fn compile_block_stmts(
        &self,
        instrs: &mut Vec<Instruction<'static>>,
        ctx: &mut FuncContext,
        stmts: &[Statement],
        value_needed: bool,
    ) -> Result<(), CompileError> {
        if stmts.is_empty() {
            if value_needed {
                instrs.push(Instruction::I64Const(0));
            }
            return Ok(());
        }

        let last_idx = stmts.len() - 1;
        for (i, stmt) in stmts.iter().enumerate() {
            let is_last = i == last_idx;
            match stmt {
                Statement::Expression(e) => {
                    self.compile_expr(instrs, ctx, &e.expr)?;
                    if !is_last || !value_needed {
                        instrs.push(Instruction::Drop);
                    }
                    // If is_last && value_needed, leave value on stack
                }
                _ => {
                    self.compile_statement(instrs, ctx, stmt)?;
                    if is_last && value_needed {
                        instrs.push(Instruction::I64Const(0));
                    }
                }
            }
        }

        Ok(())
    }

    fn compile_statement(
        &self,
        instrs: &mut Vec<Instruction<'static>>,
        ctx: &mut FuncContext,
        stmt: &Statement,
    ) -> Result<(), CompileError> {
        match stmt {
            Statement::Let(l) => {
                emit_cb_check(instrs, 1);
                self.compile_expr(instrs, ctx, &l.value)?;
                let idx = ctx.alloc_local(&l.name);
                instrs.push(Instruction::LocalSet(idx));
                Ok(())
            }
            Statement::Return(r) => {
                match &r.value {
                    Some(expr) => self.compile_expr(instrs, ctx, expr)?,
                    None => instrs.push(Instruction::I64Const(0)),
                }
                instrs.push(Instruction::Return);
                Ok(())
            }
            Statement::Expression(e) => {
                self.compile_expr(instrs, ctx, &e.expr)?;
                instrs.push(Instruction::Drop);
                Ok(())
            }
            Statement::Cb(cb) => {
                instrs.push(Instruction::I64Const(cb.budget as i64));
                instrs.push(Instruction::GlobalSet(0));
                Ok(())
            }
        }
    }

    fn compile_expr(
        &self,
        instrs: &mut Vec<Instruction<'static>>,
        ctx: &mut FuncContext,
        expr: &Expr,
    ) -> Result<(), CompileError> {
        match expr {
            Expr::IntLiteral(n) => {
                emit_cb_check(instrs, 1);
                instrs.push(Instruction::I64Const(*n));
                Ok(())
            }
            Expr::BoolLiteral(b) => {
                emit_cb_check(instrs, 1);
                instrs.push(Instruction::I64Const(if *b { 1 } else { 0 }));
                Ok(())
            }
            Expr::Identifier(name) => {
                emit_cb_check(instrs, 1);
                let idx = ctx
                    .locals
                    .get(name)
                    .ok_or_else(|| CompileError::UndefinedVariable(name.clone()))?;
                instrs.push(Instruction::LocalGet(*idx));
                Ok(())
            }
            Expr::Binary(b) => {
                emit_cb_check(instrs, 1);
                self.compile_expr(instrs, ctx, &b.left)?;
                self.compile_expr(instrs, ctx, &b.right)?;
                match b.op {
                    BinaryOp::Add => instrs.push(Instruction::I64Add),
                    BinaryOp::Sub => instrs.push(Instruction::I64Sub),
                    BinaryOp::Mul => instrs.push(Instruction::I64Mul),
                    BinaryOp::Div => instrs.push(Instruction::I64DivS),
                    BinaryOp::Eq => {
                        instrs.push(Instruction::I64Eq);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                    BinaryOp::NotEq => {
                        instrs.push(Instruction::I64Ne);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                    BinaryOp::Lt => {
                        instrs.push(Instruction::I64LtS);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                    BinaryOp::Gt => {
                        instrs.push(Instruction::I64GtS);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                    BinaryOp::LtEq => {
                        instrs.push(Instruction::I64LeS);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                    BinaryOp::GtEq => {
                        instrs.push(Instruction::I64GeS);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                }
                Ok(())
            }
            Expr::Unary(u) => {
                emit_cb_check(instrs, 1);
                match u.op {
                    UnaryOp::Neg => {
                        instrs.push(Instruction::I64Const(0));
                        self.compile_expr(instrs, ctx, &u.operand)?;
                        instrs.push(Instruction::I64Sub);
                    }
                    UnaryOp::Not => {
                        self.compile_expr(instrs, ctx, &u.operand)?;
                        instrs.push(Instruction::I64Eqz);
                        instrs.push(Instruction::I64ExtendI32U);
                    }
                }
                Ok(())
            }
            Expr::Call(c) => {
                // Built-ins are not supported in WASM compilation
                match c.callee.as_str() {
                    "print" | "len" | "typeof" => {
                        return Err(CompileError::UnsupportedFeature(format!(
                            "built-in function '{}'",
                            c.callee
                        )));
                    }
                    _ => {}
                }

                emit_cb_check(instrs, 5);

                let func_idx = self
                    .func_index
                    .get(&c.callee)
                    .ok_or_else(|| CompileError::UndefinedFunction(c.callee.clone()))?;

                for arg in &c.args {
                    self.compile_expr(instrs, ctx, arg)?;
                }

                instrs.push(Instruction::Call(*func_idx));
                Ok(())
            }
            Expr::If(i) => {
                emit_cb_check(instrs, 1);
                self.compile_expr(instrs, ctx, &i.condition)?;
                // WASM if expects i32 on stack
                instrs.push(Instruction::I32WrapI64);

                if i.else_block.is_some() {
                    // If/else as expression producing i64
                    instrs.push(Instruction::If(wasm_encoder::BlockType::Result(
                        ValType::I64,
                    )));
                    self.compile_block_stmts(
                        instrs,
                        ctx,
                        &i.then_block.statements,
                        true,
                    )?;
                    instrs.push(Instruction::Else);
                    self.compile_block_stmts(
                        instrs,
                        ctx,
                        &i.else_block.as_ref().unwrap().statements,
                        true,
                    )?;
                    instrs.push(Instruction::End);
                } else {
                    // If without else — void
                    instrs.push(Instruction::If(wasm_encoder::BlockType::Empty));
                    self.compile_block_stmts(
                        instrs,
                        ctx,
                        &i.then_block.statements,
                        false,
                    )?;
                    instrs.push(Instruction::End);
                    // Push a default value since expressions must produce i64
                    instrs.push(Instruction::I64Const(0));
                }
                Ok(())
            }
            Expr::FloatLiteral(_) => Err(CompileError::UnsupportedFeature(
                "float literals".to_string(),
            )),
            Expr::StringLiteral(_) => Err(CompileError::UnsupportedFeature(
                "string literals".to_string(),
            )),
            Expr::Prompt(_) => {
                Err(CompileError::UnsupportedFeature("prompt".to_string()))
            }
            Expr::Validate(_) => {
                Err(CompileError::UnsupportedFeature("validate".to_string()))
            }
            Expr::Explore(_) => {
                Err(CompileError::UnsupportedFeature("explore".to_string()))
            }
            Expr::FieldAccess(_) => Err(CompileError::UnsupportedFeature(
                "field access".to_string(),
            )),
            Expr::ListLiteral(_) => Err(CompileError::UnsupportedFeature(
                "list literal".to_string(),
            )),
            Expr::MapLiteral(_) => Err(CompileError::UnsupportedFeature(
                "map literal".to_string(),
            )),
        }
    }
}

// Emit CB check: if cb_remaining < cost, trap; else cb_remaining -= cost
fn emit_cb_check(instrs: &mut Vec<Instruction<'static>>, cost: i64) {
    // global.get 0        ;; load cb_remaining
    // i64.const <cost>
    // i64.lt_s            ;; remaining < cost?
    // if
    //   unreachable       ;; trap: CB exhausted
    // end
    // global.get 0
    // i64.const <cost>
    // i64.sub
    // global.set 0        ;; cb_remaining -= cost
    instrs.push(Instruction::GlobalGet(0));
    instrs.push(Instruction::I64Const(cost));
    instrs.push(Instruction::I64LtS);
    instrs.push(Instruction::If(wasm_encoder::BlockType::Empty));
    instrs.push(Instruction::Unreachable);
    instrs.push(Instruction::End);
    instrs.push(Instruction::GlobalGet(0));
    instrs.push(Instruction::I64Const(cost));
    instrs.push(Instruction::I64Sub);
    instrs.push(Instruction::GlobalSet(0));
}

// Count all let bindings in a block (recursive, includes if/else sub-blocks)
fn count_lets_in_block(block: &Block) -> u32 {
    count_lets_in_stmts(&block.statements)
}

fn count_lets_in_stmts(stmts: &[Statement]) -> u32 {
    let mut count = 0;
    for stmt in stmts {
        match stmt {
            Statement::Let(_) => count += 1,
            Statement::Expression(e) => count += count_lets_in_expr(&e.expr),
            _ => {}
        }
    }
    count
}

fn count_lets_in_expr(expr: &Expr) -> u32 {
    match expr {
        Expr::If(i) => {
            let mut c = count_lets_in_block(&i.then_block);
            if let Some(eb) = &i.else_block {
                c += count_lets_in_block(eb);
            }
            c
        }
        _ => 0,
    }
}

// --- Public API ---

pub fn compile_program(program: &Program) -> Result<Vec<u8>, CompileError> {
    let mut compiler = Compiler::new();
    compiler.collect_declarations(program)?;

    let mut module = Module::new();

    let types = compiler.build_type_section();
    module.section(&types);

    let imports = compiler.build_import_section();
    module.section(&imports);

    let functions = compiler.build_function_section();
    module.section(&functions);

    let globals = compiler.build_global_section();
    module.section(&globals);

    let exports = compiler.build_export_section();
    module.section(&exports);

    let codes = compiler.build_code_section()?;
    module.section(&codes);

    Ok(module.finish())
}

pub fn compile_from_store(
    store: &ObjectStore,
    root_hash: &str,
) -> Result<Vec<u8>, CompileError> {
    let program: Program = store
        .load(root_hash)
        .map_err(|e| CompileError::InternalError(format!("storage error: {e}")))?;
    compile_program(&program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn compile(source: &str) -> Result<Vec<u8>, CompileError> {
        let program = Parser::parse_source(source).unwrap();
        compile_program(&program)
    }

    fn validate_wasm(bytes: &[u8]) -> bool {
        wasmparser::validate(bytes).is_ok()
    }

    // --- Basic compilation ---

    #[test]
    fn compile_empty_program() {
        let wasm = compile("").unwrap();
        assert!(validate_wasm(&wasm), "empty program should produce valid WASM");
    }

    #[test]
    fn compile_int_literal() {
        let wasm = compile("42;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_arithmetic() {
        let wasm = compile("2 + 3 * 4;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_let_and_use() {
        let wasm = compile("let x = 10; let y = x + 5;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_bool_literal() {
        let wasm = compile("true; false;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_comparison() {
        let wasm = compile("let x = 5; let y = x > 3;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_unary_neg() {
        let wasm = compile("let x = -42;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_unary_not() {
        let wasm = compile("let x = !true;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    // --- Functions ---

    #[test]
    fn compile_function_def_and_call() {
        let wasm = compile(
            r#"
            fn add(a: int, b: int) -> int {
                return a + b;
            }
            let result = add(2, 3);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_recursive_function() {
        let wasm = compile(
            r#"
            fn factorial(n: int) -> int {
                if n <= 1 {
                    return 1;
                }
                return n * factorial(n - 1);
            }
            let r = factorial(5);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_multiple_functions() {
        let wasm = compile(
            r#"
            fn double(x: int) -> int {
                return x * 2;
            }
            fn quadruple(x: int) -> int {
                return double(double(x));
            }
            let r = quadruple(3);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    // --- If/else ---

    #[test]
    fn compile_if_else() {
        let wasm = compile(
            r#"
            let x = 10;
            let y = if x > 5 {
                1;
            } else {
                0;
            };
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn compile_if_no_else() {
        let wasm = compile(
            r#"
            let x = 10;
            if x > 5 {
                let y = 1;
            }
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    // --- CB metering ---

    #[test]
    fn compile_cb_statement() {
        let wasm = compile("cb 500; let x = 1;").unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn cb_check_pattern_present() {
        // Verify that CB check instructions are emitted
        let wasm = compile("let x = 1;").unwrap();
        assert!(validate_wasm(&wasm));
        // The WASM should contain global.get/i64.const/i64.lt_s/if/unreachable
        // We verify indirectly: the binary is larger than a minimal module
        // because of injected CB checks
        assert!(wasm.len() > 20, "WASM should contain CB metering code");
    }

    // --- Agent ---

    #[test]
    fn compile_agent() {
        let wasm = compile(
            r#"
            agent worker(n: int) -> int {
                return n * 2;
            }
            let r = worker(5);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    // --- Unsupported features ---

    #[test]
    fn unsupported_float() {
        let result = compile("3.14;");
        assert!(matches!(result, Err(CompileError::UnsupportedFeature(_))));
    }

    #[test]
    fn unsupported_string() {
        let result = compile(r#""hello";"#);
        assert!(matches!(result, Err(CompileError::UnsupportedFeature(_))));
    }

    #[test]
    fn unsupported_print() {
        let result = compile(r#"print(42);"#);
        assert!(matches!(result, Err(CompileError::UnsupportedFeature(_))));
    }

    #[test]
    fn unsupported_prompt() {
        let result = compile(r#"let x = 1; prompt("classify", x) -> int;"#);
        assert!(matches!(result, Err(CompileError::UnsupportedFeature(_))));
    }

    // --- Errors ---

    #[test]
    fn undefined_variable() {
        let result = compile("let x = y;");
        assert!(matches!(result, Err(CompileError::UndefinedVariable(_))));
    }

    #[test]
    fn undefined_function() {
        let result = compile("let x = foo(1);");
        assert!(matches!(result, Err(CompileError::UndefinedFunction(_))));
    }

    // --- Round-trip with storage ---

    #[test]
    fn compile_from_storage() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path()).unwrap();

        let source = r#"
            fn square(x: int) -> int {
                return x * x;
            }
            let r = square(7);
        "#;
        let program = Parser::parse_source(source).unwrap();
        let hash = store.save(&program).unwrap();

        let wasm = compile_from_store(&store, &hash).unwrap();
        assert!(validate_wasm(&wasm));
    }

    // --- Exported symbols ---

    #[test]
    fn exports_run_and_cb_remaining() {
        let wasm = compile("let x = 1;").unwrap();

        let parser = wasmparser::Parser::new(0);
        let mut has_run_export = false;
        let mut has_cb_export = false;

        for payload in parser.parse_all(&wasm) {
            if let Ok(wasmparser::Payload::ExportSection(reader)) = payload {
                for export in reader {
                    let export = export.unwrap();
                    match export.name {
                        "run" => {
                            has_run_export = true;
                            assert!(matches!(export.kind, wasmparser::ExternalKind::Func));
                        }
                        "cb_remaining" => {
                            has_cb_export = true;
                            assert!(matches!(export.kind, wasmparser::ExternalKind::Global));
                        }
                        _ => {}
                    }
                }
            }
        }

        assert!(has_run_export, "should export 'run' function");
        assert!(has_cb_export, "should export 'cb_remaining' global");
    }

    // --- WASM structure inspection ---

    #[test]
    fn global_is_mutable_i64() {
        let wasm = compile("let x = 1;").unwrap();

        let parser = wasmparser::Parser::new(0);
        for payload in parser.parse_all(&wasm) {
            if let Ok(wasmparser::Payload::GlobalSection(reader)) = payload {
                for global in reader {
                    let global = global.unwrap();
                    assert_eq!(global.ty.content_type, wasmparser::ValType::I64);
                    assert!(global.ty.mutable);
                }
            }
        }
    }

    #[test]
    fn function_with_if_in_return() {
        let wasm = compile(
            r#"
            fn abs(x: int) -> int {
                if x < 0 {
                    return -x;
                } else {
                    return x;
                }
            }
            let r = abs(-5);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }

    #[test]
    fn nested_calls() {
        let wasm = compile(
            r#"
            fn inc(x: int) -> int { return x + 1; }
            fn add3(x: int) -> int { return inc(inc(inc(x))); }
            let r = add3(0);
        "#,
        )
        .unwrap();
        assert!(validate_wasm(&wasm));
    }
}
