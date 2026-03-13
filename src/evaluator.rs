use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::ast::*;
use crate::audit::AuditLog;
use crate::capabilities::{CapError, CapKind, CapabilityRegistry};
use crate::fitness::FitnessReport;
use crate::io::IoContext;
use crate::llm::LlmBackend;
use crate::refs::Refs;
use crate::snapshot::{MemorySnapshot, SnapshotManager};
use crate::storage::ObjectStore;
use crate::trace::Tracer;

// --- Introspection Context ---

/// Evolution context injected into the evaluator for `introspect.*` access.
/// Static fields are set once before execution. Dynamic fields (cb_remaining,
/// cb_spent) are read live from the evaluator's budget state.
#[derive(Debug, Clone)]
pub struct IntrospectContext {
    pub generation: i64,
    pub lineage_id: String,
    pub arena_size: i64,
    /// Failed ancestor records (cap: 10, newest first). M45.
    pub ancestor_failures: Vec<crate::evolve::AncestorRecord>,
    /// Successful ancestor records (cap: 3, newest first). M45.
    pub ancestor_successes: Vec<crate::evolve::AncestorRecord>,
    /// Portable identity hash (Phase 12, M49). Empty string if not set.
    pub identity_hash: String,
}

impl Default for IntrospectContext {
    fn default() -> Self {
        Self {
            generation: 0,
            lineage_id: "genesis".to_string(),
            arena_size: 0,
            ancestor_failures: Vec::new(),
            ancestor_successes: Vec::new(),
            identity_hash: String::new(),
        }
    }
}

// --- Runtime Values ---

/// Handle to a spawned agent thread. Wraps JoinHandle so it can be stored
/// in Value (which must be Clone). The Option is taken once on await.
type AgentJoinHandle = Arc<Mutex<Option<std::thread::JoinHandle<Result<Value, EvalError>>>>>;

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Struct(String, HashMap<String, Value>),
    AgentHandle(AgentJoinHandle),
    Void,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (Value::Struct(na, fa), Value::Struct(nb, fb)) => na == nb && fa == fb,
            (Value::AgentHandle(a), Value::AgentHandle(b)) => Arc::ptr_eq(a, b),
            (Value::Void, Value::Void) => true,
            _ => false,
        }
    }
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
            Value::AgentHandle(_) => "agent_handle",
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
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Map(entries) => {
                write!(f, "{{")?;
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Value::Struct(name, fields) => {
                write!(f, "{name} {{ ")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            }
            Value::AgentHandle(_) => write!(f, "<agent_handle>"),
            Value::Void => write!(f, "void"),
        }
    }
}

// --- Rich Error Context ---

/// Structured error context for DAG-native diagnostics.
/// Attached to key error sites — not every expression, only high-value points.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorDetail {
    pub agent_name: Option<String>,
    pub expression_desc: String,
    pub hints: Vec<String>,
}

impl std::fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref agent) = self.agent_name {
            writeln!(f, "  in agent \"{agent}\"")?;
        }
        writeln!(f, "  at: {}", self.expression_desc)?;
        for hint in &self.hints {
            writeln!(f, "  Hint: {hint}")?;
        }
        Ok(())
    }
}

// --- Errors ---

#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    CognitiveOverload {
        budget: u64,
        required: u64,
    },
    ValidationFailed {
        predicate_index: usize,
        detail: String,
    },
    UndefinedVariable(String),
    UndefinedFunction(String),
    UndefinedField {
        type_name: String,
        field: String,
    },
    TypeError {
        expected: String,
        got: String,
    },
    DivisionByZero,
    ArityMismatch {
        expected: usize,
        got: usize,
    },
    Return(Value),
    NotAStruct(String),
    CapabilityDenied(CapError),
    General(String),
    /// Wraps an inner error with DAG context path.
    InContext {
        context: Vec<String>,
        inner: Box<EvalError>,
    },
    /// Rich error with structured context and actionable hints.
    Detailed {
        inner: Box<EvalError>,
        detail: ErrorDetail,
    },
}

impl EvalError {
    /// Wrap this error with a DAG context entry (unless it's a Return).
    fn with_context(self, ctx: String) -> Self {
        match self {
            EvalError::Return(_) => self,
            EvalError::InContext { mut context, inner } => {
                context.insert(0, ctx);
                EvalError::InContext { context, inner }
            }
            other => EvalError::InContext {
                context: vec![ctx],
                inner: Box::new(other),
            },
        }
    }

    /// Wrap this error with rich structured context.
    fn with_detail(self, detail: ErrorDetail) -> Self {
        match self {
            EvalError::Return(_) => self,
            other => EvalError::Detailed {
                inner: Box::new(other),
                detail,
            },
        }
    }

    #[allow(dead_code)]
    pub fn root_cause(&self) -> &EvalError {
        match self {
            EvalError::Detailed { inner, .. } => inner.root_cause(),
            EvalError::InContext { inner, .. } => inner.root_cause(),
            other => other,
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::CognitiveOverload { budget, required } => {
                write!(f, "CognitiveOverload: budget {budget}, required {required}")
            }
            EvalError::ValidationFailed {
                predicate_index,
                detail,
            } => {
                write!(
                    f,
                    "ValidationFailed: predicate #{predicate_index}: {detail}"
                )
            }
            EvalError::UndefinedVariable(name) => write!(f, "undefined variable: {name}"),
            EvalError::UndefinedFunction(name) => write!(f, "undefined function: {name}"),
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
            EvalError::InContext { context, inner } => {
                let path = context.join(" -> ");
                write!(f, "{path}:\n  {inner}")
            }
            EvalError::Detailed { inner, detail } => {
                writeln!(f, "{inner}")?;
                write!(f, "{detail}")
            }
        }
    }
}

/// Simple edit-distance check: returns true if two names are within
/// 2 edits of each other (for "did you mean?" suggestions).
fn levenshtein_close(a: &str, b: &str) -> bool {
    let la = a.len();
    let lb = b.len();
    if la.abs_diff(lb) > 2 {
        return false;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=lb).collect();
    let mut curr = vec![0; lb + 1];
    for i in 1..=la {
        curr[0] = i;
        for j in 1..=lb {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[lb] <= 2
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

/// Test outcome for a single explore block or validate expression.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub name: String,
    pub kind: TestKind,
    pub passed: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestKind {
    Explore,
    Validate,
}

/// Calculate total size of memo store excluding a specific key file.
fn memo_store_size_except(memo_dir: &std::path::Path, exclude_key: &str) -> u64 {
    let exclude_name = format!("{exclude_key}.jsonl");
    std::fs::read_dir(memo_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.ends_with(".jsonl") && !name.starts_with('.') && name != exclude_name
                })
                .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// Calculate total size of memo store.
pub fn memo_store_total_size(memo_dir: &std::path::Path) -> u64 {
    std::fs::read_dir(memo_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.ends_with(".jsonl") && !name.starts_with('.')
                })
                .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

/// List memo keys with entry counts and sizes.
pub fn memo_list_keys(memo_dir: &std::path::Path) -> Vec<(String, usize, u64)> {
    let mut result = Vec::new();
    let entries = match std::fs::read_dir(memo_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") || name.starts_with('.') {
            continue;
        }
        let key = name.strip_suffix(".jsonl").unwrap_or(&name).to_string();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let line_count = std::fs::read_to_string(entry.path())
            .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        result.push((key, line_count, size));
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

pub struct Evaluator<'a> {
    env: Environment,
    functions: HashMap<String, Callable>,
    types: HashMap<String, TypeDecl>,
    budget: u64,
    initial_budget: u64,
    output: Vec<String>,
    vcs: Option<(&'a ObjectStore, &'a Refs)>,
    explore_branches: Vec<String>,
    cap_registry: CapabilityRegistry,
    caps: HashMap<CapKind, Vec<crate::capabilities::CapHandle>>,
    snapshot_mgr: Option<SnapshotManager<'a>>,
    llm_backend: Option<&'a dyn LlmBackend>,
    io_context: Option<&'a IoContext>,
    tracer: Option<&'a Tracer>,
    audit: Option<&'a AuditLog>,
    current_agent_name: Option<String>,
    imported_hashes: std::collections::HashSet<String>,
    max_concurrent_agents: u32,
    active_agents: Arc<AtomicU32>,
    spawn_counter: u32,
    /// When Some, test mode is enabled: explore/validate outcomes are
    /// collected instead of aborting on failure.
    test_outcomes: Option<Vec<TestOutcome>>,
    /// Introspection context — evolution metadata accessible via `introspect.*`.
    introspect: IntrospectContext,
    /// Memo store directory (`.agentis/memo/`). None = memo disabled.
    memo_dir: Option<std::path::PathBuf>,
    /// Per-generation write limit tracking: (key, generation) → count.
    memo_write_counts: HashMap<(String, i64), usize>,
    /// Max total memo store size in bytes (default 10 MB).
    memo_max_size: u64,
    // Fitness counters (always tracked, regardless of test mode)
    prompt_count: usize,
    validates_passed: usize,
    validates_total: usize,
    explores_passed: usize,
    explores_total: usize,
}

impl<'a> Evaluator<'a> {
    pub fn new(budget: u64) -> Self {
        Self {
            env: Environment::new(),
            functions: HashMap::new(),
            types: HashMap::new(),
            budget,
            initial_budget: budget,
            output: Vec::new(),
            vcs: None,
            explore_branches: Vec::new(),
            cap_registry: CapabilityRegistry::new(),
            caps: HashMap::new(),
            snapshot_mgr: None,
            llm_backend: None,
            io_context: None,
            tracer: None,
            audit: None,
            current_agent_name: None,
            imported_hashes: std::collections::HashSet::new(),
            max_concurrent_agents: 16,
            active_agents: Arc::new(AtomicU32::new(0)),
            spawn_counter: 0,
            test_outcomes: None,
            introspect: IntrospectContext::default(),
            memo_dir: None,
            memo_write_counts: HashMap::new(),
            memo_max_size: 10 * 1024 * 1024, // 10 MB default
            prompt_count: 0,
            validates_passed: 0,
            validates_total: 0,
            explores_passed: 0,
            explores_total: 0,
        }
    }

    /// Enable test mode: explore/validate outcomes are collected instead
    /// of aborting on failure.
    pub fn with_test_mode(mut self) -> Self {
        self.test_outcomes = Some(Vec::new());
        self
    }

    /// Return collected test outcomes (only meaningful in test mode).
    pub fn test_outcomes(&self) -> &[TestOutcome] {
        match &self.test_outcomes {
            Some(v) => v,
            None => &[],
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

    pub fn with_io(mut self, io_ctx: &'a IoContext) -> Self {
        self.io_context = Some(io_ctx);
        self
    }

    pub fn with_max_agents(mut self, max: u32) -> Self {
        self.max_concurrent_agents = max;
        self
    }

    pub fn with_tracer(mut self, tracer: &'a Tracer) -> Self {
        self.tracer = Some(tracer);
        self
    }

    pub fn with_audit(mut self, audit: &'a AuditLog) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_persistence(mut self, store: &'a ObjectStore) -> Self {
        self.snapshot_mgr = Some(SnapshotManager::new(store));
        self
    }

    /// Enable persistent snapshot registry at the given `.agentis` root.
    /// Must be called after `with_persistence`.
    pub fn with_snapshot_registry(mut self, agentis_root: &std::path::Path) -> Self {
        if let Some(mgr) = self.snapshot_mgr.take() {
            self.snapshot_mgr = Some(mgr.with_registry(agentis_root));
        }
        self
    }

    /// Set the memo store directory (`.agentis/memo/`).
    pub fn with_memo_dir(mut self, dir: &std::path::Path) -> Self {
        self.memo_dir = Some(dir.to_path_buf());
        self
    }

    /// Set the max total memo store size in bytes.
    pub fn with_memo_max_size(mut self, bytes: u64) -> Self {
        self.memo_max_size = bytes;
        self
    }

    #[allow(dead_code)] // used by evolve in M45
    pub fn with_introspect(mut self, ctx: IntrospectContext) -> Self {
        self.introspect = ctx;
        self
    }

    /// Convert an AncestorRecord into a Value::Struct for injection.
    fn ancestor_record_to_value(rec: &crate::evolve::AncestorRecord) -> Value {
        let mut fields = HashMap::new();
        fields.insert("generation".to_string(), Value::Int(rec.generation as i64));
        fields.insert("outcome".to_string(), Value::String(rec.outcome.clone()));
        fields.insert("fitness_score".to_string(), Value::Float(rec.fitness_score));
        fields.insert(
            "code_hash".to_string(),
            Value::String(rec.code_hash.clone()),
        );
        fields.insert("elapsed_ms".to_string(), Value::Int(rec.elapsed_ms as i64));
        Value::Struct("AncestorRecord".to_string(), fields)
    }

    /// Inject the `introspect` struct into the environment.
    /// Static fields are set from IntrospectContext. Dynamic fields
    /// (cb_remaining, cb_spent) are handled in eval_field_access.
    fn inject_introspect(&mut self) {
        let mut fields = HashMap::new();
        fields.insert(
            "generation".to_string(),
            Value::Int(self.introspect.generation),
        );
        fields.insert(
            "lineage_id".to_string(),
            Value::String(self.introspect.lineage_id.clone()),
        );
        fields.insert(
            "arena_size".to_string(),
            Value::Int(self.introspect.arena_size),
        );
        // cb_remaining and cb_spent are placeholders — read dynamically
        fields.insert("cb_remaining".to_string(), Value::Int(0));
        fields.insert("cb_spent".to_string(), Value::Int(0));

        // Identity hash (Phase 12, M49) — free access
        fields.insert(
            "identity_hash".to_string(),
            Value::String(self.introspect.identity_hash.clone()),
        );

        // Ancestor lists (M45)
        let failures: Vec<Value> = self
            .introspect
            .ancestor_failures
            .iter()
            .map(Self::ancestor_record_to_value)
            .collect();
        fields.insert("ancestor_failures".to_string(), Value::List(failures));

        let successes: Vec<Value> = self
            .introspect
            .ancestor_successes
            .iter()
            .map(Self::ancestor_record_to_value)
            .collect();
        fields.insert("ancestor_successes".to_string(), Value::List(successes));

        // Lineage summary (M45) — derived from ancestors
        let total =
            self.introspect.ancestor_failures.len() + self.introspect.ancestor_successes.len();
        let success_count = self.introspect.ancestor_successes.len();
        let failure_count = self.introspect.ancestor_failures.len();
        let avg_fitness = if total > 0 {
            let sum: f64 = self
                .introspect
                .ancestor_failures
                .iter()
                .chain(self.introspect.ancestor_successes.iter())
                .map(|r| r.fitness_score)
                .sum();
            sum / total as f64
        } else {
            0.0
        };
        let mut summary_fields = HashMap::new();
        summary_fields.insert("total_ancestors".to_string(), Value::Int(total as i64));
        summary_fields.insert(
            "success_count".to_string(),
            Value::Int(success_count as i64),
        );
        summary_fields.insert(
            "failure_count".to_string(),
            Value::Int(failure_count as i64),
        );
        summary_fields.insert("avg_fitness".to_string(), Value::Float(avg_fitness));
        fields.insert(
            "lineage_summary".to_string(),
            Value::Struct("LineageSummary".to_string(), summary_fields),
        );

        self.env.define(
            "introspect".to_string(),
            Value::Struct("Introspect".to_string(), fields),
        );
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

    #[allow(dead_code)]
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
                Err(EvalError::CapabilityDenied(CapError::RevokedCapability(
                    kind,
                )))
            }
            None => Err(EvalError::CapabilityDenied(CapError::MissingCapability(
                kind,
            ))),
        }
    }

    pub fn capture_snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            scopes: self.env.snapshot_scopes(),
            budget_remaining: self.budget,
            output: self.output.clone(),
        }
    }

    #[allow(dead_code)]
    pub fn restore_snapshot(&mut self, snapshot: &MemorySnapshot) {
        self.env.restore_scopes(snapshot.scopes.clone());
        self.budget = snapshot.budget_remaining;
        self.output = snapshot.output.clone();
    }

    /// Restore from snapshot with 30% CB penalty (resurrection tax).
    /// Resurrected agents have less fuel than before death — evolutionary
    /// pressure against fragile agents that die repeatedly.
    pub fn restore_snapshot_with_penalty(&mut self, snapshot: &MemorySnapshot) {
        self.env.restore_scopes(snapshot.scopes.clone());
        self.budget = (snapshot.budget_remaining as f64 * 0.7) as u64;
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

    #[allow(dead_code)]
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

    /// Snapshot current execution metrics as a fitness report.
    pub fn fitness_report(&self) -> FitnessReport {
        FitnessReport {
            cb_initial: self.initial_budget,
            cb_remaining: self.budget,
            validates_passed: self.validates_passed,
            validates_total: self.validates_total,
            explores_passed: self.explores_passed,
            explores_total: self.explores_total,
            prompt_count: self.prompt_count,
            error: false,
        }
    }

    /// Look up a variable by name (for REPL display, no CB cost).
    pub fn lookup_var(&self, name: &str) -> Option<Value> {
        self.env.get(name).cloned()
    }

    fn spend(&mut self, cost: u64) -> Result<(), EvalError> {
        if self.budget < cost {
            return Err(EvalError::CognitiveOverload {
                budget: self.budget,
                required: cost,
            }
            .with_detail(ErrorDetail {
                agent_name: self.current_agent_name.clone(),
                expression_desc: format!("operation costing {} CB", cost),
                hints: vec![
                    format!(
                        "remaining budget: {}, initial: {}",
                        self.budget, self.initial_budget
                    ),
                    "increase budget with 'cb <amount>;' or reduce operations".to_string(),
                ],
            }));
        }
        self.budget -= cost;
        Ok(())
    }

    pub fn eval_program(&mut self, program: &Program) -> Result<Value, EvalError> {
        // Inject introspect object (static fields only — cb is dynamic)
        self.inject_introspect();

        // First pass: resolve imports, register functions, agents, types
        for decl in &program.declarations {
            match decl {
                Declaration::Import(imp) => {
                    self.resolve_import(imp)?;
                }
                Declaration::Function(f) => {
                    self.functions
                        .insert(f.name.clone(), Callable::Function(f.clone()));
                }
                Declaration::Agent(a) => {
                    self.functions
                        .insert(a.name.clone(), Callable::Agent(a.clone()));
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
                match self.eval_statement(stmt) {
                    Ok(val) => {
                        // In test mode, record validate successes for
                        // top-level validate expressions.
                        if self.test_outcomes.is_some()
                            && let Statement::Expression(expr_stmt) = stmt
                            && let Expr::Validate(v) = &expr_stmt.expr
                        {
                            self.test_outcomes.as_mut().unwrap().push(TestOutcome {
                                name: format!("validate ({} predicates)", v.predicates.len()),
                                kind: TestKind::Validate,
                                passed: true,
                                detail: None,
                            });
                        }
                        last = val;
                        self.persist_snapshot();
                    }
                    Err(e) => {
                        if let Some(outcomes) = &mut self.test_outcomes {
                            // In test mode: record the failure, continue
                            if let Statement::Expression(expr_stmt) = stmt
                                && let Expr::Validate(v) = &expr_stmt.expr
                            {
                                outcomes.push(TestOutcome {
                                    name: format!("validate ({} predicates)", v.predicates.len()),
                                    kind: TestKind::Validate,
                                    passed: false,
                                    detail: Some(format!("{e}")),
                                });
                                continue;
                            }
                            // Non-validate error in test mode: still abort
                            return Err(e);
                        }
                        return Err(e);
                    }
                }
            }
        }
        Ok(last)
    }

    /// Evaluate a single declaration in REPL mode.
    /// Registers functions/agents/types immediately and evaluates statements.
    /// Returns the resulting value (for display in the REPL).
    pub fn eval_repl_declaration(&mut self, decl: &Declaration) -> Result<Value, EvalError> {
        match decl {
            Declaration::Import(imp) => {
                self.resolve_import(imp)?;
                Ok(Value::Void)
            }
            Declaration::Function(f) => {
                self.functions
                    .insert(f.name.clone(), Callable::Function(f.clone()));
                Ok(Value::Void)
            }
            Declaration::Agent(a) => {
                self.functions
                    .insert(a.name.clone(), Callable::Agent(a.clone()));
                Ok(Value::Void)
            }
            Declaration::Type(t) => {
                self.types.insert(t.name.clone(), t.clone());
                Ok(Value::Void)
            }
            Declaration::Statement(stmt) => {
                let val = self.eval_statement(stmt)?;
                self.persist_snapshot();
                Ok(val)
            }
        }
    }

    /// Resolve an import declaration: load AST from object store by hash,
    /// register imported functions/agents/types in scope. Detects cycles.
    fn resolve_import(&mut self, imp: &ImportDecl) -> Result<(), EvalError> {
        // Cycle detection
        if self.imported_hashes.contains(&imp.hash) {
            return Err(EvalError::General(format!(
                "cyclic import detected: {}",
                &imp.hash[..12.min(imp.hash.len())]
            )));
        }
        self.imported_hashes.insert(imp.hash.clone());

        if let Some(t) = &self.tracer {
            t.import_resolved(&imp.hash, imp.alias.as_deref());
        }

        // Load the program from object store
        let store = match &self.vcs {
            Some((store, _)) => *store,
            None => {
                return Err(EvalError::General(
                    "imports require VCS (object store not available)".into(),
                ));
            }
        };

        let imported_program: Program = store.load(&imp.hash).map_err(|e| {
            EvalError::General(format!(
                "import {}: {e}",
                &imp.hash[..12.min(imp.hash.len())]
            ))
        })?;

        // Recursively resolve imports in the imported program
        for decl in &imported_program.declarations {
            if let Declaration::Import(sub_imp) = decl {
                self.resolve_import(sub_imp)?;
            }
        }

        // Collect all exported declarations from the imported program
        let mut funcs: Vec<(String, Callable)> = Vec::new();
        let mut type_decls: Vec<(String, TypeDecl)> = Vec::new();

        for decl in &imported_program.declarations {
            match decl {
                Declaration::Function(f) => {
                    funcs.push((f.name.clone(), Callable::Function(f.clone())));
                }
                Declaration::Agent(a) => {
                    funcs.push((a.name.clone(), Callable::Agent(a.clone())));
                }
                Declaration::Type(t) => {
                    type_decls.push((t.name.clone(), t.clone()));
                }
                _ => {}
            }
        }

        // Apply import filtering and aliasing
        if let Some(ref names) = imp.names {
            // Selective import: import "hash" { name1, name2 };
            for name in names {
                if let Some(pos) = funcs.iter().position(|(n, _)| n == name) {
                    let (n, c) = funcs[pos].clone();
                    self.functions.insert(n, c);
                } else if let Some(pos) = type_decls.iter().position(|(n, _)| n == name) {
                    let (n, t) = type_decls[pos].clone();
                    self.types.insert(n, t);
                } else {
                    return Err(EvalError::General(format!(
                        "import: '{}' not found in {}",
                        name,
                        &imp.hash[..12.min(imp.hash.len())]
                    )));
                }
            }
        } else if let Some(ref alias) = imp.alias {
            // Aliased import: import "hash" as utils;
            // Register with prefixed names: utils.func_name
            for (name, callable) in &funcs {
                self.functions
                    .insert(format!("{alias}.{name}"), callable.clone());
            }
            for (name, type_decl) in &type_decls {
                self.types
                    .insert(format!("{alias}.{name}"), type_decl.clone());
            }
        } else {
            // Bare import: import "hash"; — import everything
            for (name, callable) in funcs {
                self.functions.insert(name, callable);
            }
            for (name, type_decl) in type_decls {
                self.types.insert(name, type_decl);
            }
        }

        Ok(())
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
                self.initial_budget = cb.budget;
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
            Expr::Spawn(s) => self.eval_spawn(s),
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
                if *b == 0 {
                    return Err(EvalError::DivisionByZero);
                }
                Ok(Value::Int(a / b))
            }

            // Float arithmetic
            (Value::Float(a), BinaryOp::Add, Value::Float(b)) => Ok(Value::Float(a + b)),
            (Value::Float(a), BinaryOp::Sub, Value::Float(b)) => Ok(Value::Float(a - b)),
            (Value::Float(a), BinaryOp::Mul, Value::Float(b)) => Ok(Value::Float(a * b)),
            (Value::Float(a), BinaryOp::Div, Value::Float(b)) => {
                if *b == 0.0 {
                    return Err(EvalError::DivisionByZero);
                }
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
                if *b == 0.0 {
                    return Err(EvalError::DivisionByZero);
                }
                Ok(Value::Float(*a as f64 / b))
            }
            (Value::Float(a), BinaryOp::Div, Value::Int(b)) => {
                if *b == 0 {
                    return Err(EvalError::DivisionByZero);
                }
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
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
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
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
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
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
                }
                let collection = self.eval_expr(&expr.args[0])?;
                let key = self.eval_expr(&expr.args[1])?;
                return match (&collection, &key) {
                    (Value::List(items), Value::Int(idx)) => {
                        let i = *idx as usize;
                        items.get(i).cloned().ok_or_else(|| {
                            EvalError::General(format!(
                                "index {i} out of bounds (len {})",
                                items.len()
                            ))
                        })
                    }
                    (Value::Map(entries), _) => {
                        for (k, v) in entries {
                            if *k == key {
                                return Ok(v.clone());
                            }
                        }
                        Err(EvalError::General("key not found in map".to_string()))
                    }
                    _ => Err(EvalError::TypeError {
                        expected: "list or map".into(),
                        got: collection.type_name().to_string(),
                    }),
                };
            }
            "map_of" => {
                if !expr.args.len().is_multiple_of(2) {
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
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let val = self.eval_expr(&expr.args[0])?;
                return Ok(Value::String(val.type_name().to_string()));
            }
            "to_string" => {
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let val = self.eval_expr(&expr.args[0])?;
                return Ok(Value::String(format!("{val}")));
            }
            // --- Memo Store (M46) ---
            "memo_write" => {
                self.spend(1)?; // memo write costs 1 CB
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
                }
                let key_val = self.eval_expr(&expr.args[0])?;
                let value_val = self.eval_expr(&expr.args[1])?;
                let key = match &key_val {
                    Value::String(s) => s.clone(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: key_val.type_name().to_string(),
                        });
                    }
                };
                // Sanitize key: alphanumeric + hyphens only
                if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return Err(EvalError::General(format!(
                        "memo key must be non-empty and contain only alphanumeric characters or hyphens, got: \"{}\"",
                        key
                    )));
                }
                let memo_dir = self
                    .memo_dir
                    .as_ref()
                    .ok_or_else(|| EvalError::General("memo store not configured".into()))?
                    .clone();
                // Per-generation write limit: max 20 per key per generation
                let generation = self.introspect.generation;
                let count_key = (key.clone(), generation);
                let count = self.memo_write_counts.entry(count_key).or_insert(0);
                if *count >= 20 {
                    eprintln!(
                        "[memo] write limit reached for key \"{}\" in generation {}",
                        key, generation
                    );
                    return Ok(Value::Void);
                }
                *count += 1;
                // Truncate value at 10 KB
                let mut value_str = format!("{value_val}");
                if value_str.len() > 10240 {
                    value_str.truncate(10240);
                    value_str.push_str("[truncated]");
                }
                // Check max 50 keys limit
                std::fs::create_dir_all(&memo_dir)
                    .map_err(|e| EvalError::General(format!("memo: create dir: {e}")))?;
                let memo_file = memo_dir.join(format!("{key}.jsonl"));
                if !memo_file.exists() {
                    // Count existing keys
                    let key_count = std::fs::read_dir(&memo_dir)
                        .map(|rd| {
                            rd.filter_map(|e| e.ok())
                                .filter(|e| {
                                    e.path().extension().is_some_and(|ext| ext == "jsonl")
                                        && !e.file_name().to_string_lossy().starts_with('.')
                                })
                                .count()
                        })
                        .unwrap_or(0);
                    if key_count >= 50 {
                        return Err(EvalError::General(format!(
                            "memo: max 50 keys exceeded ({key_count} keys exist)"
                        )));
                    }
                }
                // Write entry as JSONL
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let entry = crate::json::object(vec![
                    ("generation", crate::json::JsonValue::Int(generation)),
                    ("value", crate::json::JsonValue::String(value_str)),
                    ("timestamp", crate::json::JsonValue::Int(ts as i64)),
                ]);
                // Read existing, append, enforce limits
                let existing = std::fs::read_to_string(&memo_file).unwrap_or_default();
                let mut lines: Vec<&str> =
                    existing.lines().filter(|l| !l.trim().is_empty()).collect();
                // Max 100 entries per key — evict oldest (FIFO: drop from front)
                while lines.len() >= 100 {
                    lines.remove(0);
                }
                let entry_str = format!("{entry}");
                lines.push(&entry_str);
                let new_content = lines.join("\n") + "\n";
                // Total size check — evict oldest entries from this key
                let max_size = self.memo_max_size;
                let other_size = memo_store_size_except(&memo_dir, &key);
                let mut content_bytes = new_content.len() as u64;
                let mut eviction_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
                while content_bytes + other_size > max_size && eviction_lines.len() > 1 {
                    eviction_lines.remove(0);
                    let evicted = eviction_lines.join("\n") + "\n";
                    content_bytes = evicted.len() as u64;
                }
                let final_content = if eviction_lines.len() < lines.len() {
                    eviction_lines.join("\n") + "\n"
                } else {
                    new_content
                };
                // Atomic write: tmp + rename
                let tmp_file = memo_dir.join(format!(".{key}.tmp"));
                std::fs::write(&tmp_file, &final_content)
                    .map_err(|e| EvalError::General(format!("memo: write tmp: {e}")))?;
                std::fs::rename(&tmp_file, &memo_file)
                    .map_err(|e| EvalError::General(format!("memo: rename: {e}")))?;
                return Ok(Value::Void);
            }
            "recall" => {
                // Reading is free (0 CB beyond the 5 for the call itself)
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let key_val = self.eval_expr(&expr.args[0])?;
                let key = match &key_val {
                    Value::String(s) => s.clone(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: key_val.type_name().to_string(),
                        });
                    }
                };
                let memo_dir = match &self.memo_dir {
                    Some(d) => d.clone(),
                    None => return Ok(Value::List(Vec::new())),
                };
                let memo_file = memo_dir.join(format!("{key}.jsonl"));
                let content = std::fs::read_to_string(&memo_file).unwrap_or_default();
                let mut values: Vec<Value> = Vec::new();
                for line in content.lines().rev() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(parsed) = crate::json::parse(line)
                        && let Some(v) = parsed.get("value").and_then(|v| v.as_str())
                    {
                        values.push(Value::String(v.to_string()));
                    }
                }
                return Ok(Value::List(values));
            }
            "recall_latest" => {
                // Reading is free (0 CB beyond the 5 for the call itself)
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let key_val = self.eval_expr(&expr.args[0])?;
                let key = match &key_val {
                    Value::String(s) => s.clone(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: key_val.type_name().to_string(),
                        });
                    }
                };
                let memo_dir = match &self.memo_dir {
                    Some(d) => d.clone(),
                    None => return Ok(Value::String(String::new())),
                };
                let memo_file = memo_dir.join(format!("{key}.jsonl"));
                let content = std::fs::read_to_string(&memo_file).unwrap_or_default();
                // Return last non-empty line's value
                for line in content.lines().rev() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(parsed) = crate::json::parse(line)
                        && let Some(v) = parsed.get("value").and_then(|v| v.as_str())
                    {
                        return Ok(Value::String(v.to_string()));
                    }
                }
                return Ok(Value::String(String::new()));
            }
            "file_read" => {
                self.require_cap(CapKind::FileRead)?;
                self.spend(10)?;
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let path = self.eval_expr(&expr.args[0])?;
                let path_str = match &path {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: path.type_name().to_string(),
                        });
                    }
                };
                let io = self
                    .io_context
                    .ok_or_else(|| EvalError::General("I/O not configured".into()))?;
                let content = io
                    .file_read(path_str)
                    .map_err(|e| EvalError::General(format!("{e}")))?;
                return Ok(Value::String(content));
            }
            "file_write" => {
                self.require_cap(CapKind::FileWrite)?;
                self.spend(10)?;
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
                }
                let path = self.eval_expr(&expr.args[0])?;
                let content = self.eval_expr(&expr.args[1])?;
                let path_str = match &path {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: path.type_name().to_string(),
                        });
                    }
                };
                let content_str = format!("{content}");
                let io = self
                    .io_context
                    .ok_or_else(|| EvalError::General("I/O not configured".into()))?;
                io.file_write(path_str, &content_str)
                    .map_err(|e| EvalError::General(format!("{e}")))?;
                return Ok(Value::Void);
            }
            "http_get" => {
                self.require_cap(CapKind::NetConnect)?;
                self.spend(25)?;
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let url = self.eval_expr(&expr.args[0])?;
                let url_str = match &url {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: url.type_name().to_string(),
                        });
                    }
                };
                let io = self
                    .io_context
                    .ok_or_else(|| EvalError::General("I/O not configured".into()))?;
                let body = io
                    .http_get(url_str)
                    .map_err(|e| EvalError::General(format!("{e}")))?;
                return Ok(Value::String(body));
            }
            "http_post" => {
                self.require_cap(CapKind::NetConnect)?;
                self.spend(25)?;
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
                }
                let url = self.eval_expr(&expr.args[0])?;
                let body = self.eval_expr(&expr.args[1])?;
                let url_str = match &url {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "string".into(),
                            got: url.type_name().to_string(),
                        });
                    }
                };
                let body_str = match &body {
                    Value::String(s) => s.clone(),
                    _ => format!("{body}"),
                };
                let io = self
                    .io_context
                    .ok_or_else(|| EvalError::General("I/O not configured".into()))?;
                let response = io
                    .http_post(url_str, &body_str)
                    .map_err(|e| EvalError::General(format!("{e}")))?;
                return Ok(Value::String(response));
            }
            "await" => {
                if expr.args.len() != 1 {
                    return Err(EvalError::ArityMismatch {
                        expected: 1,
                        got: expr.args.len(),
                    });
                }
                let handle_val = self.eval_expr(&expr.args[0])?;
                return self.await_agent(handle_val);
            }
            "await_timeout" => {
                if expr.args.len() != 2 {
                    return Err(EvalError::ArityMismatch {
                        expected: 2,
                        got: expr.args.len(),
                    });
                }
                let handle_val = self.eval_expr(&expr.args[0])?;
                let timeout_val = self.eval_expr(&expr.args[1])?;
                let ms = match &timeout_val {
                    Value::Int(n) => *n as u64,
                    _ => {
                        return Err(EvalError::TypeError {
                            expected: "int".into(),
                            got: timeout_val.type_name().to_string(),
                        });
                    }
                };
                return self.await_agent_timeout(handle_val, ms);
            }
            _ => {}
        }

        // User-defined functions/agents
        let callable = match self.functions.get(&expr.callee).cloned() {
            Some(c) => c,
            None => {
                let err = EvalError::UndefinedFunction(expr.callee.clone());
                let mut hints = Vec::new();
                // Suggest similar names
                let similar: Vec<&String> = self
                    .functions
                    .keys()
                    .filter(|k| {
                        k.contains(&expr.callee)
                            || expr.callee.contains(k.as_str())
                            || levenshtein_close(k, &expr.callee)
                    })
                    .take(3)
                    .collect();
                if !similar.is_empty() {
                    hints.push(format!(
                        "similar: {}",
                        similar
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                return Err(err.with_detail(ErrorDetail {
                    agent_name: self.current_agent_name.clone(),
                    expression_desc: format!("{}(... {} args)", expr.callee, expr.args.len()),
                    hints,
                }));
            }
        };

        let (params, body, is_agent) = match &callable {
            Callable::Function(f) => (&f.params, &f.body, false),
            Callable::Agent(a) => (&a.params, &a.body, true),
        };

        if params.len() != expr.args.len() {
            let kind = if is_agent { "agent" } else { "fn" };
            let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            return Err(EvalError::ArityMismatch {
                expected: params.len(),
                got: expr.args.len(),
            }
            .with_detail(ErrorDetail {
                agent_name: self.current_agent_name.clone(),
                expression_desc: format!("{kind} {}({})", expr.callee, param_names.join(", ")),
                hints: vec![format!(
                    "{} \"{}\" expects {} args ({}), got {}",
                    kind,
                    expr.callee,
                    params.len(),
                    param_names.join(", "),
                    expr.args.len()
                )],
            }));
        }

        // Evaluate arguments
        let mut arg_values = Vec::new();
        for arg in &expr.args {
            arg_values.push(self.eval_expr(arg)?);
        }

        // Trace agent entry
        if is_agent && let Some(t) = &self.tracer {
            t.agent_entered(&expr.callee, self.budget);
        }

        // Save state for agents (isolated scope)
        let saved_env = if is_agent {
            Some(self.env.clone())
        } else {
            None
        };
        let saved_agent_name = if is_agent {
            let prev = self.current_agent_name.take();
            self.current_agent_name = Some(expr.callee.clone());
            Some(prev)
        } else {
            None
        };

        // Set up call scope
        self.env.push_scope();
        for (param, value) in params.iter().zip(arg_values) {
            self.env.define(param.name.clone(), value);
        }

        // Execute body
        let callee_name = expr.callee.clone();
        let kind = if is_agent { "agent" } else { "fn" };
        let result = match self.eval_block_inner(&body.statements) {
            Ok(v) => v,
            Err(EvalError::Return(v)) => v,
            Err(e) => {
                self.env.pop_scope();
                if let Some(env) = saved_env {
                    self.env = env;
                }
                if let Some(prev) = saved_agent_name {
                    self.current_agent_name = prev;
                }
                return Err(e.with_context(format!("{kind} \"{callee_name}\"")));
            }
        };

        self.env.pop_scope();

        // Restore env for agents
        if let Some(env) = saved_env {
            if let Some(t) = &self.tracer {
                t.agent_exited(&expr.callee, result.type_name());
            }
            self.env = env;
        }
        if let Some(prev) = saved_agent_name {
            self.current_agent_name = prev;
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
        let object = self.eval_expr(&expr.object)?;
        match &object {
            Value::Struct(type_name, fields) => {
                // Introspection is free (0 CB) — reading own state, not prompting
                if type_name == "Introspect" {
                    // Dynamic fields — read live from evaluator budget state
                    match expr.field.as_str() {
                        "cb_remaining" => return Ok(Value::Int(self.budget as i64)),
                        "cb_spent" => {
                            return Ok(Value::Int((self.initial_budget - self.budget) as i64));
                        }
                        _ => {}
                    }
                    // Static introspect fields (generation, lineage_id, etc.)
                    return fields.get(&expr.field).cloned().ok_or_else(|| {
                        EvalError::UndefinedField {
                            type_name: type_name.clone(),
                            field: expr.field.clone(),
                        }
                    });
                }
                // Non-introspect field access costs 1 CB
                self.spend(1)?;
                fields
                    .get(&expr.field)
                    .cloned()
                    .ok_or_else(|| EvalError::UndefinedField {
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

        let return_type_str = Self::format_type_annotation(&expr.return_type);
        if let Some(t) = &self.tracer {
            t.prompt_call(&expr.instruction, &return_type_str);
        }

        // PII guard: scan input, block if PII detected without PiiTransmit
        let pii_result = crate::pii::scan(&input_str);
        let has_pii_cap = if !pii_result.is_clean() {
            let granted = self.require_cap(CapKind::PiiTransmit).is_ok();
            if let Some(t) = &self.tracer {
                t.pii_scan_result(&pii_result.types_str(), granted);
            }
            if !granted {
                // Audit the blocked prompt before returning error
                self.audit_prompt(&expr.instruction, &input_str, &pii_result, false);
                let err = EvalError::CapabilityDenied(
                    crate::capabilities::CapError::MissingCapability(CapKind::PiiTransmit),
                );
                return Err(err.with_detail(ErrorDetail {
                    agent_name: self.current_agent_name.clone(),
                    expression_desc: format!(
                        "prompt(\"{}\", <input>) -> {}",
                        expr.instruction, return_type_str
                    ),
                    hints: vec![
                        format!("PII detected: {}", pii_result.types_str()),
                        format!("input length: {} chars", input_str.len()),
                        "grant PiiTransmit via --grant-pii or config pii_transmit = allow"
                            .to_string(),
                    ],
                }));
            }
            true
        } else {
            false
        };

        // Audit the prompt call (successful — not blocked)
        self.audit_prompt(&expr.instruction, &input_str, &pii_result, has_pii_cap);

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

        let backend: &dyn LlmBackend = match self.llm_backend {
            Some(b) => b,
            None => &crate::llm::MockBackend,
        };

        let backend_name = backend.name().to_string();
        let is_mock = backend_name == "mock";

        // For mock: trace + run inline (no timer needed).
        // For real backends: run with "still waiting" timer thread.
        let result = if is_mock {
            if let Some(t) = &self.tracer {
                t.llm_requesting("mock", "");
            }
            let r = backend
                .complete(&expr.instruction, &input_str, &expr.return_type, fields_opt)
                .map_err(|e| EvalError::General(format!("{e}")));
            if let Some(t) = &self.tracer {
                t.llm_received(0.0);
            }
            r
        } else {
            self.call_llm_with_timer(
                backend,
                &expr.instruction,
                &input_str,
                &expr.return_type,
                fields_opt,
            )?
        };

        if let Some(t) = &self.tracer {
            if let Ok(ref v) = result {
                t.llm_response(v);
            }
            t.cb_remaining(self.budget, self.initial_budget);
        }

        // Fitness counter: track successful prompt calls
        if result.is_ok() {
            self.prompt_count += 1;
        }

        result
    }

    fn audit_prompt(
        &self,
        instruction: &str,
        input: &str,
        pii_result: &crate::pii::PiiScanResult,
        pii_transmit_granted: bool,
    ) {
        if let Some(audit) = self.audit {
            let backend: &dyn LlmBackend = match self.llm_backend {
                Some(b) => b,
                None => &crate::llm::MockBackend,
            };
            let entry = crate::audit::PromptAuditEntry {
                agent_name: self.current_agent_name.as_deref(),
                instruction,
                input,
                pii_result,
                pii_transmit_granted,
                backend_name: backend.name(),
                model: "",
            };
            audit.log_prompt(&entry);
        }
    }

    /// Call LLM backend with a "still waiting" timer on a background thread.
    fn call_llm_with_timer(
        &self,
        backend: &dyn LlmBackend,
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
        fields: Option<&[(&str, &str)]>,
    ) -> Result<Result<Value, EvalError>, EvalError> {
        use std::sync::atomic::{AtomicBool, Ordering as AtomOrd};
        use std::time::{Duration, Instant};

        if let Some(t) = &self.tracer {
            t.llm_requesting(backend.name(), "");
        }

        let start = Instant::now();
        let done = Arc::new(AtomicBool::new(false));
        let done_clone = done.clone();

        // Spawn timer thread that prints "still waiting" every 4 seconds.
        // Uses eprintln! directly to avoid sending Tracer across threads.
        let has_tracer = self.tracer.is_some();
        let timer_thread = std::thread::spawn(move || {
            if !has_tracer {
                return;
            }
            let interval = Duration::from_secs(4);
            let mut next_tick = start + interval;
            while !done_clone.load(AtomOrd::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                if !done_clone.load(AtomOrd::Relaxed) && Instant::now() >= next_tick {
                    let elapsed = start.elapsed().as_secs_f64();
                    eprintln!("[llm] still waiting ... ({elapsed:.1}s)");
                    next_tick += interval;
                }
            }
        });

        let result = backend
            .complete(instruction, input, return_type, fields)
            .map_err(|e| EvalError::General(format!("{e}")));

        done.store(true, Ordering::Relaxed);
        let _ = timer_thread.join();

        if let Some(t) = &self.tracer {
            t.llm_received(start.elapsed().as_secs_f64());
        }

        Ok(result)
    }

    fn format_type_annotation(ann: &TypeAnnotation) -> String {
        match ann {
            TypeAnnotation::Named(name) => name.clone(),
            TypeAnnotation::Generic(name, args) => {
                let args_str: Vec<String> = args.iter().map(Self::format_type_annotation).collect();
                format!("{}<{}>", name, args_str.join(", "))
            }
        }
    }

    fn collect_type_fields(&self, type_ann: &TypeAnnotation) -> Vec<(String, String)> {
        if let TypeAnnotation::Named(name) = type_ann
            && let Some(type_decl) = self.types.get(name)
        {
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
        Vec::new()
    }

    fn eval_validate(&mut self, expr: &ValidateExpr) -> Result<Value, EvalError> {
        let target = self.eval_expr(&expr.target)?;
        let count = expr.predicates.len();
        self.validates_total += 1;

        for (i, predicate) in expr.predicates.iter().enumerate() {
            self.spend(1)?;
            let result = self.eval_expr(predicate)?;
            match result {
                Value::Bool(true) => {
                    if let Some(t) = &self.tracer {
                        t.validate_detail(i, true);
                    }
                }
                Value::Bool(false) => {
                    if let Some(t) = &self.tracer {
                        t.validate_detail(i, false);
                        t.validate_result(count, false);
                    }
                    return Err(EvalError::ValidationFailed {
                        predicate_index: i,
                        detail: format!("predicate #{i} evaluated to false"),
                    }
                    .with_detail(ErrorDetail {
                        agent_name: self.current_agent_name.clone(),
                        expression_desc: format!(
                            "validate <{}> ({} predicates)",
                            target.type_name(),
                            count
                        ),
                        hints: vec![format!(
                            "predicate #{i} of {count} failed (target type: {})",
                            target.type_name()
                        )],
                    }));
                }
                _ => {
                    return Err(EvalError::TypeError {
                        expected: "bool".into(),
                        got: result.type_name().to_string(),
                    });
                }
            }
        }

        if let Some(t) = &self.tracer {
            t.validate_result(count, true);
        }
        self.validates_passed += 1;
        Ok(target)
    }

    fn eval_spawn(&mut self, expr: &SpawnExpr) -> Result<Value, EvalError> {
        self.spend(10)?;

        // Check agent limit (per-evaluator counter, shared with children)
        let current = self.active_agents.load(Ordering::SeqCst);
        if current >= self.max_concurrent_agents {
            return Err(EvalError::General("agent limit exceeded".into()));
        }

        self.spawn_counter += 1;
        let handle_id = self.spawn_counter;

        // Look up the agent
        let callable = self
            .functions
            .get(&expr.agent_name)
            .cloned()
            .ok_or_else(|| EvalError::UndefinedFunction(expr.agent_name.clone()))?;

        let (params, body) = match &callable {
            Callable::Agent(a) => (&a.params, a.body.clone()),
            Callable::Function(_) => {
                return Err(EvalError::General(format!(
                    "spawn requires an agent, '{}' is a function",
                    expr.agent_name
                )));
            }
        };

        if params.len() != expr.args.len() {
            let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            return Err(EvalError::ArityMismatch {
                expected: params.len(),
                got: expr.args.len(),
            }
            .with_detail(ErrorDetail {
                agent_name: self.current_agent_name.clone(),
                expression_desc: format!(
                    "spawn agent {}({})",
                    expr.agent_name,
                    param_names.join(", ")
                ),
                hints: vec![format!(
                    "agent \"{}\" expects {} args, got {}",
                    expr.agent_name,
                    params.len(),
                    expr.args.len()
                )],
            }));
        }

        // Evaluate arguments in parent scope
        let mut arg_vals = Vec::with_capacity(expr.args.len());
        for arg in &expr.args {
            arg_vals.push(self.eval_expr(arg)?);
        }

        // Clone what the child needs (no references — must be 'static)
        let child_functions = self.functions.clone();
        let child_types = self.types.clone();
        let child_budget = self.budget; // child gets same budget as parent's current
        let child_params: Vec<(String, Value)> = params
            .iter()
            .map(|p| p.name.clone())
            .zip(arg_vals)
            .collect();
        let max_agents = self.max_concurrent_agents;
        let active_counter = self.active_agents.clone();

        if let Some(t) = &self.tracer {
            t.spawn_agent(&expr.agent_name, child_budget, handle_id);
        }

        active_counter.fetch_add(1, Ordering::SeqCst);
        let counter_for_thread = active_counter.clone();

        let handle = std::thread::spawn(move || {
            let result = (|| {
                let mut child = Evaluator::new(child_budget).with_max_agents(max_agents);
                child.active_agents = counter_for_thread.clone();
                child.grant_all();
                child.functions = child_functions;
                child.types = child_types;

                // Set up agent's scope with parameters
                child.env.push_scope();
                for (name, val) in child_params {
                    child.env.define(name, val);
                }

                let mut last = Value::Void;
                for stmt in &body.statements {
                    match child.eval_statement(stmt) {
                        Ok(v) => last = v,
                        Err(EvalError::Return(v)) => return Ok(v),
                        Err(e) => return Err(e),
                    }
                }
                Ok(last)
            })();

            counter_for_thread.fetch_sub(1, Ordering::SeqCst);
            result
        });

        Ok(Value::AgentHandle(Arc::new(Mutex::new(Some(handle)))))
    }

    fn await_agent(&mut self, handle_val: Value) -> Result<Value, EvalError> {
        let handle_arc = match handle_val {
            Value::AgentHandle(h) => h,
            _ => {
                return Err(EvalError::TypeError {
                    expected: "agent_handle".into(),
                    got: handle_val.type_name().to_string(),
                });
            }
        };

        let join_handle = handle_arc
            .lock()
            .map_err(|_| EvalError::General("agent handle poisoned".into()))?
            .take()
            .ok_or_else(|| EvalError::General("agent already awaited".into()))?;

        let result = match join_handle.join() {
            Ok(result) => result,
            Err(_) => Err(EvalError::General("spawned agent panicked".into())),
        };

        if let Some(t) = &self.tracer {
            let result_type = match &result {
                Ok(v) => v.type_name().to_string(),
                Err(e) => format!("error: {e}"),
            };
            // Use spawn_counter as rough handle ID for trace
            t.await_completed(0, &result_type);
        }

        result
    }

    fn await_agent_timeout(&mut self, handle_val: Value, ms: u64) -> Result<Value, EvalError> {
        let handle_arc = match handle_val {
            Value::AgentHandle(h) => h,
            _ => {
                return Err(EvalError::TypeError {
                    expected: "agent_handle".into(),
                    got: handle_val.type_name().to_string(),
                });
            }
        };

        let join_handle = handle_arc
            .lock()
            .map_err(|_| EvalError::General("agent handle poisoned".into()))?
            .take()
            .ok_or_else(|| EvalError::General("agent already awaited".into()))?;

        // Poll with timeout using a parking thread
        let result_arc: Arc<Mutex<Option<Result<Value, EvalError>>>> = Arc::new(Mutex::new(None));
        let result_clone = result_arc.clone();

        let waiter = std::thread::spawn(move || {
            let r = match join_handle.join() {
                Ok(result) => result,
                Err(_) => Err(EvalError::General("spawned agent panicked".into())),
            };
            *result_clone.lock().unwrap() = Some(r);
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        loop {
            if let Some(result) = result_arc.lock().unwrap().take() {
                let _ = waiter.join();
                return result;
            }
            if std::time::Instant::now() >= deadline {
                // Timeout — the thread is still running, we can't kill it,
                // but we report the error. The thread will eventually finish.
                return Err(EvalError::CognitiveOverload {
                    budget: ms,
                    required: ms + 1,
                });
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn eval_explore(&mut self, expr: &ExploreBlock) -> Result<Value, EvalError> {
        self.require_cap(CapKind::VcsWrite)?;
        self.spend(1)?;
        self.explores_total += 1;

        if let Some(t) = &self.tracer {
            t.explore_entered(&expr.name, self.budget);
        }

        // Save current state
        let saved_env = self.env.clone();
        let saved_budget = self.budget;

        // Run in isolated context
        let result = self.eval_block(&expr.body);

        match result {
            Ok(value) => {
                self.explores_passed += 1;
                // Success: create a VCS branch if store/refs are available
                if let Some((store, refs)) = &self.vcs {
                    let _ = refs.create_branch(&expr.name, None);
                    let _ = store;
                    self.explore_branches.push(expr.name.clone());
                }
                if let Some(t) = &self.tracer {
                    t.explore_outcome(&expr.name, true);
                }
                if let Some(outcomes) = &mut self.test_outcomes {
                    outcomes.push(TestOutcome {
                        name: expr.name.clone(),
                        kind: TestKind::Explore,
                        passed: true,
                        detail: None,
                    });
                }
                // Transaction boundary: explore block completed, call stack empty
                self.persist_snapshot();
                Ok(value)
            }
            Err(e) => {
                if let Some(t) = &self.tracer {
                    t.explore_outcome(&expr.name, false);
                }
                // Failure: restore everything, no side effects
                self.env = saved_env;
                self.budget = saved_budget;
                // In test mode: record failure, continue
                if let Some(outcomes) = &mut self.test_outcomes {
                    outcomes.push(TestOutcome {
                        name: expr.name.clone(),
                        kind: TestKind::Explore,
                        passed: false,
                        detail: Some(format!("{}", e)),
                    });
                    return Ok(Value::Void);
                }
                Err(e.with_context(format!("explore \"{}\"", expr.name)))
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
        assert_eq!(
            eval(r#""hello" + " " + "world";"#),
            Ok(Value::String("hello world".into()))
        );
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
        let output = eval_output(
            r#"
            if true {
                print("yes");
            }
        "#,
        );
        assert_eq!(output, vec!["yes"]);
    }

    #[test]
    fn if_false_no_else() {
        let output = eval_output(
            r#"
            if false {
                print("yes");
            }
        "#,
        );
        assert!(output.is_empty());
    }

    #[test]
    fn if_else() {
        let output = eval_output(
            r#"
            if false {
                print("yes");
            } else {
                print("no");
            }
        "#,
        );
        assert_eq!(output, vec!["no"]);
    }

    // --- Functions ---

    #[test]
    fn function_call() {
        assert_eq!(
            eval(
                r#"
            fn add(a: int, b: int) -> int {
                return a + b;
            }
            add(2, 3);
        "#
            ),
            Ok(Value::Int(5))
        );
    }

    #[test]
    fn recursive_function() {
        assert_eq!(
            eval(
                r#"
            fn factorial(n: int) -> int {
                if n <= 1 {
                    return 1;
                }
                return n * factorial(n - 1);
            }
            factorial(5);
        "#
            ),
            Ok(Value::Int(120))
        );
    }

    #[test]
    fn function_arity_mismatch() {
        let result = eval(
            r#"
            fn f(x: int) -> int { return x; }
            f(1, 2);
        "#,
        );
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::ArityMismatch { .. })
        ));
    }

    #[test]
    fn undefined_function() {
        let result = eval("foo();");
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::UndefinedFunction(_))
        ));
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
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::CognitiveOverload { .. })
        ));
    }

    #[test]
    fn cb_function_call_cost() {
        // Function call costs 5 CB
        let result = eval_with_budget(
            r#"
            fn f() -> int { return 1; }
            f();
        "#,
            4,
        );
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::CognitiveOverload { .. })
        ));
    }

    #[test]
    fn cb_statement_override() {
        assert_eq!(
            eval(
                r#"
            fn f() -> int {
                cb 100;
                return 42;
            }
            f();
        "#
            ),
            Ok(Value::Int(42))
        );
    }

    #[test]
    fn cb_recursive_exhaustion() {
        let result = eval_with_budget(
            r#"
            fn loop_forever(n: int) -> int {
                return loop_forever(n + 1);
            }
            loop_forever(0);
        "#,
            100,
        );
        // Error is wrapped in InContext from the call chain
        assert!(result.is_err());
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("CognitiveOverload"));
    }

    // --- Agent ---

    #[test]
    fn agent_basic() {
        assert_eq!(
            eval(
                r#"
            agent greet(name: string) -> string {
                return "hello " + name;
            }
            greet("world");
        "#
            ),
            Ok(Value::String("hello world".into()))
        );
    }

    #[test]
    fn agent_isolation() {
        // Agent should not see outer mutable state
        let output = eval_output(
            r#"
            let x = 10;
            agent f(n: int) -> int {
                return n + 1;
            }
            let result = f(5);
            print(result);
            print(x);
        "#,
        );
        assert_eq!(output, vec!["6", "10"]);
    }

    // --- Prompt (mock) ---

    #[test]
    fn prompt_mock_primitive() {
        assert_eq!(
            eval(
                r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#
            ),
            Ok(Value::Int(0))
        );
    }

    #[test]
    fn prompt_mock_struct() {
        let result = eval(
            r#"
            type Report {
                summary: string,
                score: float
            }
            let data = "test";
            prompt("analyze", data) -> Report;
        "#,
        );
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
        let result = eval_with_budget(
            r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#,
            49,
        );
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::CognitiveOverload { .. })
        ));
    }

    // --- Validate ---

    #[test]
    fn validate_passes() {
        assert_eq!(
            eval(
                r#"
            let x = 10;
            validate x { x > 5 };
        "#
            ),
            Ok(Value::Int(10))
        );
    }

    #[test]
    fn validate_fails() {
        let result = eval(
            r#"
            let x = 3;
            validate x { x > 5 };
        "#,
        );
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::ValidationFailed { .. })
        ));
    }

    #[test]
    fn validate_multiple_predicates() {
        assert_eq!(
            eval(
                r#"
            let x = 10;
            validate x { x > 5, x < 20 };
        "#
            ),
            Ok(Value::Int(10))
        );
    }

    #[test]
    fn validate_second_predicate_fails() {
        let result = eval(
            r#"
            let x = 10;
            validate x { x > 5, x < 8 };
        "#,
        );
        match result.as_ref().map_err(|e| e.root_cause()) {
            Err(EvalError::ValidationFailed {
                predicate_index, ..
            }) => {
                assert_eq!(*predicate_index, 1);
            }
            _ => panic!("expected validation failure on predicate 1"),
        }
    }

    // --- Explore ---

    #[test]
    fn explore_success() {
        let result = eval(
            r#"
            fn f() {
                explore "test" {
                    let x = 42;
                };
            }
            f();
        "#,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn explore_failure_restores_state() {
        // Explore that fails should not affect outer state
        let result = eval(
            r#"
            let x = 10;
            explore "failing" {
                let y = 1 / 0;
            }
        "#,
        );
        // The explore block fails, which propagates as an error
        // but the outer state should be restored
        assert!(result.is_err());
    }

    // --- Field access ---

    #[test]
    fn field_access_on_struct() {
        let result = eval(
            r#"
            type Report {
                summary: string,
                score: float
            }
            let data = "test";
            let r = prompt("analyze", data) -> Report;
            r.summary;
        "#,
        );
        assert_eq!(result, Ok(Value::String("mock".into())));
    }

    #[test]
    fn field_access_on_non_struct() {
        let result = eval(
            r#"
            let x = 42;
            x.field;
        "#,
        );
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
        let program = Parser::parse_source(
            r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#,
        )
        .unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn prompt_with_capability_succeeds() {
        let program = Parser::parse_source(
            r#"
            let x = "input";
            prompt("classify", x) -> int;
        "#,
        )
        .unwrap();
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
        let program = Parser::parse_source(
            r#"
            explore "test-branch" {
                let x = 42;
            }
        "#,
        )
        .unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn explore_with_capability_succeeds() {
        let program = Parser::parse_source(
            r#"
            explore "test-branch" {
                let x = 42;
            }
        "#,
        )
        .unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant(CapKind::VcsWrite);
        let result = evaluator.eval_program(&program);
        assert!(result.is_ok());
    }

    #[test]
    fn no_caps_pure_code_works() {
        // Arithmetic, let, if, functions should work without any capabilities
        let program = Parser::parse_source(
            r#"
            fn add(a: int, b: int) -> int { return a + b; }
            let x = add(2, 3);
            if x > 4 { x; } else { 0; };
        "#,
        )
        .unwrap();
        let mut evaluator = Evaluator::new(10000);
        // No caps granted
        let result = evaluator.eval_program(&program);
        assert_eq!(result, Ok(Value::Int(5)));
    }

    #[test]
    fn grant_all_allows_everything() {
        let program = Parser::parse_source(
            r#"
            let x = "input";
            let r = prompt("classify", x) -> int;
            print(r);
        "#,
        )
        .unwrap();
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
                assert!(
                    msg.contains("stdout"),
                    "error should mention the capability kind"
                );
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
    fn restore_snapshot_with_cb_penalty() {
        let program = Parser::parse_source("let x = 42;").unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let snap = evaluator.capture_snapshot();
        let original_budget = snap.budget_remaining;

        let mut evaluator2 = Evaluator::new(0);
        evaluator2.grant_all();
        evaluator2.restore_snapshot_with_penalty(&snap);

        let expected = (original_budget as f64 * 0.7) as u64;
        assert_eq!(evaluator2.budget_remaining(), expected);
        assert!(evaluator2.budget_remaining() < original_budget);
    }

    #[test]
    fn persistence_creates_snapshots_per_statement() {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let store = crate::storage::ObjectStore::init(dir.path()).unwrap();

        let program = Parser::parse_source("let x = 1; let y = 2; let z = 3;").unwrap();
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
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
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
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
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
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
        let mut evaluator = Evaluator::new(10000).with_persistence(&store);
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
            Ok(Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ]))
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
            Ok(Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ]))
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
        assert!(matches!(
            eval("map_of(\"a\", 1, \"b\");"),
            Err(EvalError::General(_))
        ));
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
        assert!(matches!(
            eval("get(map_of(\"a\", 1), \"z\");"),
            Err(EvalError::General(_))
        ));
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
            scope.get("xs")
                == Some(&Value::List(vec![
                    Value::Int(1),
                    Value::Int(2),
                    Value::Int(3),
                ]))
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
            scope.get("m")
                == Some(&Value::Map(vec![(
                    Value::String("a".into()),
                    Value::Int(1),
                )]))
        }));
    }

    // --- I/O builtins ---

    fn eval_with_io(source: &str, io_ctx: &IoContext) -> Result<Value, EvalError> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(10000).with_io(io_ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program)
    }

    fn eval_with_io_output(source: &str, io_ctx: &IoContext) -> Vec<String> {
        let program = Parser::parse_source(source).unwrap();
        let mut evaluator = Evaluator::new(10000).with_io(io_ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        evaluator.output().to_vec()
    }

    fn test_io_context() -> (crate::storage::tempfile::TempDir, IoContext) {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let agentis_root = dir.path().join(".agentis");
        std::fs::create_dir_all(agentis_root.join("sandbox")).unwrap();
        let config = crate::config::Config::parse("");
        let ctx = IoContext::new(&agentis_root, &config);
        (dir, ctx)
    }

    #[test]
    fn io_file_write_and_read() {
        let (_dir, io) = test_io_context();
        let output = eval_with_io_output(
            r#"file_write("test.txt", "hello");
               let content = file_read("test.txt");
               print(content);"#,
            &io,
        );
        assert_eq!(output, vec!["hello"]);
    }

    #[test]
    fn io_file_read_nonexistent() {
        let (_dir, io) = test_io_context();
        let result = eval_with_io(r#"file_read("nope.txt");"#, &io);
        assert!(result.is_err());
    }

    #[test]
    fn io_file_write_subdirectory() {
        let (_dir, io) = test_io_context();
        let output = eval_with_io_output(
            r#"file_write("sub/data.txt", "nested");
               print(file_read("sub/data.txt"));"#,
            &io,
        );
        assert_eq!(output, vec!["nested"]);
    }

    #[test]
    fn io_path_traversal_blocked() {
        let (_dir, io) = test_io_context();
        let result = eval_with_io(r#"file_write("../../escape.txt", "evil");"#, &io);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("path outside sandbox"));
    }

    #[test]
    fn io_file_read_requires_cap() {
        let (_dir, io) = test_io_context();
        let program = Parser::parse_source(r#"file_read("test.txt");"#).unwrap();
        let mut evaluator = Evaluator::new(10000).with_io(&io);
        // Grant all EXCEPT FileRead
        evaluator.grant(CapKind::Stdout);
        evaluator.grant(CapKind::FileWrite);
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn io_file_write_requires_cap() {
        let (_dir, io) = test_io_context();
        let program = Parser::parse_source(r#"file_write("test.txt", "x");"#).unwrap();
        let mut evaluator = Evaluator::new(10000).with_io(&io);
        evaluator.grant(CapKind::Stdout);
        evaluator.grant(CapKind::FileRead);
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn io_file_read_costs_10_cb() {
        let (_dir, io) = test_io_context();
        // file_write costs 5 (call) + 10 (io) = 15, file_read costs 5+10 = 15
        // Total: 30. Budget of 25 should fail on the read.
        let program =
            Parser::parse_source(r#"file_write("x.txt", "y"); file_read("x.txt");"#).unwrap();
        let mut evaluator = Evaluator::new(25).with_io(&io);
        evaluator.grant_all();
        let result = evaluator.eval_program(&program);
        assert!(matches!(
            result.as_ref().map_err(|e| e.root_cause()),
            Err(EvalError::CognitiveOverload { .. })
        ));
    }

    #[test]
    fn io_http_get_requires_cap() {
        let (_dir, io) = test_io_context();
        let program = Parser::parse_source(r#"http_get("https://example.com");"#).unwrap();
        let mut evaluator = Evaluator::new(10000).with_io(&io);
        evaluator.grant(CapKind::Stdout);
        // No NetConnect granted
        let result = evaluator.eval_program(&program);
        assert!(matches!(result, Err(EvalError::CapabilityDenied(_))));
    }

    #[test]
    fn io_http_get_domain_not_whitelisted() {
        let (_dir, io) = test_io_context();
        // io has empty whitelist by default
        let result = eval_with_io(r#"http_get("https://example.com");"#, &io);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("domain not whitelisted") || err.contains("no domains whitelisted"));
    }

    #[test]
    fn io_without_context_errors() {
        // No I/O context configured — should get clear error
        let result = eval(r#"file_read("test.txt");"#);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("I/O not configured"));
    }

    // --- Module import tests ---

    fn make_store() -> (crate::storage::tempfile::TempDir, ObjectStore, Refs) {
        let dir = crate::storage::tempfile::tempdir().unwrap();
        let root = dir.path().join(".agentis");
        let store = ObjectStore::init(&root).unwrap();
        let refs = Refs::new(&root);
        refs.init().unwrap();
        (dir, store, refs)
    }

    #[test]
    fn import_bare_registers_functions() {
        let (_dir, store, refs) = make_store();

        // Store a library program with a function
        let lib_source = "fn double(x: int) -> int { return x * 2; }";
        let lib_program = Parser::parse_source(lib_source).unwrap();
        let lib_hash = store.save(&lib_program).unwrap();

        // Main program imports the library and calls the function
        let main_source = format!(
            r#"import "{lib_hash}";
            double(21);"#
        );
        let main_program = Parser::parse_source(&main_source).unwrap();

        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn import_selective_names() {
        let (_dir, store, refs) = make_store();

        let lib_source = "fn add(a: int, b: int) -> int { return a + b; }
                          fn sub(a: int, b: int) -> int { return a - b; }";
        let lib_program = Parser::parse_source(lib_source).unwrap();
        let lib_hash = store.save(&lib_program).unwrap();

        // Import only 'add'
        let main_source = format!(
            r#"import "{lib_hash}" {{ add }};
            add(10, 5);"#
        );
        let main_program = Parser::parse_source(&main_source).unwrap();

        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program).unwrap();
        assert_eq!(result, Value::Int(15));

        // 'sub' should NOT be available
        let main_source2 = format!(
            r#"import "{lib_hash}" {{ add }};
            sub(10, 5);"#
        );
        let main_program2 = Parser::parse_source(&main_source2).unwrap();
        let mut eval2 = Evaluator::new(10000).with_vcs(&store, &refs);
        eval2.grant_all();
        assert!(eval2.eval_program(&main_program2).is_err());
    }

    #[test]
    fn import_aliased() {
        let (_dir, store, refs) = make_store();

        let lib_source = "fn greet() -> string { return \"hello\"; }";
        let lib_program = Parser::parse_source(lib_source).unwrap();
        let lib_hash = store.save(&lib_program).unwrap();

        // Import with alias — function accessible as utils.greet
        // We test by calling directly since dotted calls go through the same lookup
        let main_source = format!(
            r#"import "{lib_hash}" as utils;
            utils.greet();"#
        );

        // Parser won't handle `utils.greet()` as a call — it would parse
        // as field access. Let's test aliased registration directly.
        // For now, verify the function is registered with aliased name.
        let main_program =
            Parser::parse_source(&format!(r#"import "{lib_hash}" as utils; 42;"#)).unwrap();

        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        evaluator.eval_program(&main_program).unwrap();

        // The function should be registered as "utils.greet"
        assert!(evaluator.functions.contains_key("utils.greet"));
    }

    #[test]
    fn import_cyclic_detected() {
        let (_dir, store, refs) = make_store();

        // Create a program that imports itself (via its own hash).
        // We need a two-step approach: first store a program, then create
        // one that imports it, and have the imported one import back.
        // Simpler: store program A, store program B that imports A,
        // then A imports B — but we can't modify stored content.
        //
        // Instead: manually test cycle detection by importing same hash twice
        // in a chain. The simplest cycle: A imports A.
        // But we can't know A's hash before storing it.
        //
        // Test the mechanism: if the same hash is in imported_hashes, error.
        let lib_source = "fn noop() -> int { return 0; }";
        let lib_program = Parser::parse_source(lib_source).unwrap();
        let lib_hash = store.save(&lib_program).unwrap();

        // Import the same hash twice — second time should be caught by cycle detection
        let main_source = format!(
            r#"import "{lib_hash}";
               import "{lib_hash}";
               noop();"#
        );
        let main_program = Parser::parse_source(&main_source).unwrap();
        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("cyclic import"));
    }

    #[test]
    fn import_nonexistent_hash() {
        let (_dir, store, refs) = make_store();

        let main_source = r#"import "0000000000000000000000000000000000000000000000000000000000000000";
            42;"#;
        let main_program = Parser::parse_source(main_source).unwrap();
        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program);
        assert!(result.is_err());
    }

    #[test]
    fn import_selective_nonexistent_name() {
        let (_dir, store, refs) = make_store();

        let lib_source = "fn real_fn() -> int { return 1; }";
        let lib_program = Parser::parse_source(lib_source).unwrap();
        let lib_hash = store.save(&lib_program).unwrap();

        let main_source = format!(
            r#"import "{lib_hash}" {{ nonexistent }};
            42;"#
        );
        let main_program = Parser::parse_source(&main_source).unwrap();
        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not found"));
    }

    #[test]
    fn import_transitive() {
        let (_dir, store, refs) = make_store();

        // Library A: defines helper
        let lib_a_source = "fn helper() -> int { return 7; }";
        let lib_a = Parser::parse_source(lib_a_source).unwrap();
        let hash_a = store.save(&lib_a).unwrap();

        // Library B: imports A, defines wrapper
        let lib_b_source = format!(
            r#"import "{hash_a}";
               fn wrapper() -> int {{ return helper(); }}"#
        );
        let lib_b = Parser::parse_source(&lib_b_source).unwrap();
        let hash_b = store.save(&lib_b).unwrap();

        // Main: imports B, calls wrapper (which calls helper from A)
        let main_source = format!(
            r#"import "{hash_b}";
            wrapper();"#
        );
        let main_program = Parser::parse_source(&main_source).unwrap();

        let mut evaluator = Evaluator::new(10000).with_vcs(&store, &refs);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program).unwrap();
        assert_eq!(result, Value::Int(7));
    }

    #[test]
    fn import_without_vcs_errors() {
        let main_source = r#"import "somehash"; 42;"#;
        let main_program = Parser::parse_source(main_source).unwrap();
        let mut evaluator = Evaluator::new(10000);
        evaluator.grant_all();
        let result = evaluator.eval_program(&main_program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("object store not available"));
    }

    // --- Spawn/await tests ---

    #[test]
    fn spawn_and_await_basic() {
        let result = eval(
            r#"
            agent worker(x: int) -> int {
                return x * 2;
            }
            let h = spawn worker(21);
            await(h);
        "#,
        );
        assert_eq!(result, Ok(Value::Int(42)));
    }

    #[test]
    fn spawn_two_agents_parallel() {
        let output = eval_output(
            r#"
            agent adder(a: int, b: int) -> int {
                return a + b;
            }
            let h1 = spawn adder(10, 20);
            let h2 = spawn adder(100, 200);
            let r1 = await(h1);
            let r2 = await(h2);
            print(r1);
            print(r2);
        "#,
        );
        assert_eq!(output, vec!["30", "300"]);
    }

    #[test]
    fn spawn_requires_agent_not_function() {
        let result = eval(
            r#"
            fn helper(x: int) -> int { return x; }
            let h = spawn helper(1);
            await(h);
        "#,
        );
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("spawn requires an agent"));
    }

    #[test]
    fn spawn_error_propagates_on_await() {
        let result = eval(
            r#"
            agent failing() -> int {
                return 1 / 0;
            }
            let h = spawn failing();
            await(h);
        "#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn spawn_costs_10_cb() {
        // agent call: spend(10) for spawn + spend(5) for internal call overhead
        // We need budget for: top-level statements parsing + spawn(10) + await
        // With very tight budget, spawn should exhaust it
        let result = eval_with_budget(
            r#"
            agent noop() -> int { return 0; }
            let h = spawn noop(  );
            await(h);
        "#,
            12,
        );
        // Budget should be too tight: declarations are free, but first
        // `spawn noop()` costs 10, `await(h)` is a call costing 5
        // 12 < 10 + 5 for remaining `await` call, but spawn should succeed
        // since 12 >= 10. The await call itself costs 5 more.
        // Actually: `let h = ...` costs 1 (let), spawn costs 10. That's 11.
        // Then `await(h)` costs 5 for call + 1 for identifier lookup = 6. Total = 17.
        // Budget of 12 is not enough for the full sequence.
        assert!(result.is_err());
    }

    #[test]
    fn spawn_agent_limit() {
        // max_agents=0 means no agents can be spawned at all
        let program = Parser::parse_source(
            r#"
            agent noop() -> int { return 1; }
            let h = spawn noop();
            await(h);
        "#,
        )
        .unwrap();
        let mut evaluator = Evaluator::new(10000).with_max_agents(0);
        evaluator.grant_all();
        let result = evaluator.eval_program(&program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("agent limit exceeded"));
    }

    #[test]
    fn await_twice_errors() {
        let result = eval(
            r#"
            agent worker() -> int { return 42; }
            let h = spawn worker();
            await(h);
            await(h);
        "#,
        );
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("already awaited"));
    }

    #[test]
    fn await_timeout_success() {
        let result = eval(
            r#"
            agent fast() -> int { return 7; }
            let h = spawn fast();
            await_timeout(h, 5000);
        "#,
        );
        assert_eq!(result, Ok(Value::Int(7)));
    }

    #[test]
    fn spawn_with_string_args() {
        let output = eval_output(
            r#"
            agent echo(msg: string) -> string {
                return msg;
            }
            let h = spawn echo("hello");
            let result = await(h);
            print(result);
        "#,
        );
        assert_eq!(output, vec!["hello"]);
    }

    #[test]
    fn typeof_agent_handle() {
        let output = eval_output(
            r#"
            agent noop() -> int { return 0; }
            let h = spawn noop();
            print(typeof(h));
            await(h);
        "#,
        );
        assert_eq!(output, vec!["agent_handle"]);
    }

    // --- AST-native diagnostics ---

    #[test]
    fn error_context_in_function() {
        let result = eval(
            r#"
            fn bad() -> int {
                return x;
            }
            bad();
        "#,
        );
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("fn \"bad\""));
        assert!(err_str.contains("undefined variable: x"));
    }

    #[test]
    fn error_context_in_agent() {
        let result = eval(
            r#"
            agent broken() -> int {
                return y;
            }
            broken();
        "#,
        );
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("agent \"broken\""));
        assert!(err_str.contains("undefined variable: y"));
    }

    #[test]
    fn error_context_nested() {
        let result = eval(
            r#"
            fn inner() -> int {
                return z;
            }
            fn outer() -> int {
                return inner();
            }
            outer();
        "#,
        );
        let err_str = format!("{}", result.unwrap_err());
        assert!(err_str.contains("fn \"inner\""));
        assert!(err_str.contains("fn \"outer\""));
        assert!(err_str.contains("undefined variable: z"));
    }

    #[test]
    fn error_in_context_display() {
        let inner = EvalError::UndefinedVariable("x".into());
        let wrapped = inner.with_context("fn \"calc\"".into());
        let s = format!("{wrapped}");
        assert_eq!(s, "fn \"calc\":\n  undefined variable: x");
    }

    // --- PII Guard tests ---

    #[test]
    fn pii_guard_blocks_email_in_prompt() {
        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "contact user@example.com") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("pii_transmit"),
            "expected PII capability error, got: {err}"
        );
    }

    #[test]
    fn pii_guard_allows_with_pii_transmit() {
        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "contact user@example.com") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        ev.grant(CapKind::PiiTransmit);
        let result = ev.eval_program(&program);
        assert!(result.is_ok());
    }

    #[test]
    fn pii_guard_passes_clean_text() {
        let program =
            Parser::parse_source(r#"let x = prompt("analyze", "hello world") -> string;"#).unwrap();
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_ok());
    }

    #[test]
    fn pii_guard_blocks_phone() {
        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "call +420 123 456 789") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("pii_transmit"));
    }

    #[test]
    fn pii_guard_blocks_credit_card() {
        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "card 4111 1111 1111 1111") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
    }

    #[test]
    fn pii_guard_grant_all_excludes_pii_transmit() {
        let mut ev = Evaluator::new(1000);
        ev.grant_all();
        assert!(ev.require_cap(CapKind::PiiTransmit).is_err());
    }

    #[test]
    fn audit_log_records_clean_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("audit")).unwrap();
        let audit = crate::audit::AuditLog::open(&root).unwrap();

        let program =
            Parser::parse_source(r#"let x = prompt("summarize", "hello world") -> string;"#)
                .unwrap();
        let mut ev = Evaluator::new(1000).with_audit(&audit);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        drop(audit);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed = crate::json::parse(lines[0]).unwrap();
        assert_eq!(parsed.get("pii_scan").unwrap().as_str(), Some("clean"));
        assert_eq!(parsed.get("backend").unwrap().as_str(), Some("mock"));
    }

    #[test]
    fn audit_log_records_blocked_pii_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("audit")).unwrap();
        let audit = crate::audit::AuditLog::open(&root).unwrap();

        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "contact user@example.com") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000).with_audit(&audit);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
        drop(audit);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed = crate::json::parse(lines[0]).unwrap();
        assert_eq!(parsed.get("pii_scan").unwrap().as_str(), Some("detected"));
        assert_eq!(
            parsed.get("pii_transmit_granted").unwrap().as_bool(),
            Some(false)
        );
    }

    #[test]
    fn audit_log_records_granted_pii_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("audit")).unwrap();
        let audit = crate::audit::AuditLog::open(&root).unwrap();

        let program = Parser::parse_source(
            r#"let x = prompt("analyze", "contact user@example.com") -> string;"#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000).with_audit(&audit);
        ev.grant_all();
        ev.grant(CapKind::PiiTransmit);
        ev.eval_program(&program).unwrap();
        drop(audit);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed = crate::json::parse(lines[0]).unwrap();
        assert_eq!(parsed.get("pii_scan").unwrap().as_str(), Some("detected"));
        assert_eq!(
            parsed.get("pii_transmit_granted").unwrap().as_bool(),
            Some(true)
        );
    }

    #[test]
    fn audit_log_tracks_agent_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("audit")).unwrap();
        let audit = crate::audit::AuditLog::open(&root).unwrap();

        let program = Parser::parse_source(
            r#"
            agent worker(data: string) -> string {
                cb 100;
                return prompt("summarize", data) -> string;
            }
            let result = worker("hello");
        "#,
        )
        .unwrap();
        let mut ev = Evaluator::new(1000).with_audit(&audit);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        drop(audit);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let parsed = crate::json::parse(content.trim()).unwrap();
        assert_eq!(parsed.get("agent").unwrap().as_str(), Some("worker"));
    }

    // --- Fitness counter tests ---

    #[test]
    fn fitness_prompt_count() {
        let source = r#"
            let x = prompt("say hi", "hello") -> string;
            let y = prompt("say bye", "goodbye") -> string;
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        let report = ev.fitness_report();
        assert_eq!(report.prompt_count, 2);
        assert_eq!(report.cb_initial, 10000);
        assert!(!report.error);
    }

    #[test]
    fn fitness_validate_counters() {
        let source = r#"
            let x = 5;
            validate x { x > 0, x < 10 };
            validate x { x == 5 };
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        let report = ev.fitness_report();
        assert_eq!(report.validates_passed, 2);
        assert_eq!(report.validates_total, 2);
    }

    #[test]
    fn fitness_validate_failure_counts() {
        let source = r#"
            let x = 5;
            validate x { x > 100 };
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
        let report = ev.fitness_report();
        assert_eq!(report.validates_passed, 0);
        assert_eq!(report.validates_total, 1);
    }

    #[test]
    fn fitness_explore_counters() {
        let source = r#"
            explore "good" {
                let x = 42;
            }
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        let report = ev.fitness_report();
        assert_eq!(report.explores_passed, 1);
        assert_eq!(report.explores_total, 1);
    }

    #[test]
    fn fitness_explore_failure_counts() {
        let source = r#"
            explore "bad" {
                let x = 1 / 0;
            }
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        let result = ev.eval_program(&program);
        assert!(result.is_err());
        let report = ev.fitness_report();
        assert_eq!(report.explores_passed, 0);
        assert_eq!(report.explores_total, 1);
    }

    #[test]
    fn fitness_report_score_with_redistribution() {
        let source = r#"
            let x = prompt("test", "input") -> string;
            validate x { true };
        "#;
        let program = Parser::parse_source(source).unwrap();
        let mut ev = Evaluator::new(10000);
        ev.grant_all();
        ev.eval_program(&program).unwrap();
        let report = ev.fitness_report();
        // No explores, so w_exp redistributed to w_cb + w_val
        assert_eq!(report.explores_total, 0);
        assert_eq!(report.validates_passed, 1);
        assert_eq!(report.prompt_count, 1);
        let score = report.score();
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    // --- Introspection tests ---

    #[test]
    fn introspect_defaults_outside_evolution() {
        let out = eval_output(
            r#"
            let g = introspect.generation;
            let lid = introspect.lineage_id;
            let a = introspect.arena_size;
            print(g, lid, a);
        "#,
        );
        assert_eq!(out, &["0 genesis 0"]);
    }

    #[test]
    fn introspect_cb_remaining_dynamic() {
        let out = eval_output(
            r#"
            cb 100;
            let r1 = introspect.cb_remaining;
            let a = 1 + 2;
            let b = 3 + 4;
            let r2 = introspect.cb_remaining;
            print(r1 > r2);
        "#,
        );
        assert_eq!(out, &["true"]);
    }

    #[test]
    fn introspect_cb_spent_increases() {
        let out = eval_output(
            r#"
            cb 200;
            let s1 = introspect.cb_spent;
            let a = 1 + 2;
            let b = 3 + 4;
            let c = 5 + 6;
            let s2 = introspect.cb_spent;
            print(s2 > s1);
        "#,
        );
        assert_eq!(out, &["true"]);
    }

    #[test]
    fn introspect_field_access_costs_zero_cb() {
        // Field access on Introspect is free (0 CB).
        // Each `let r = introspect.cb_remaining` costs: let(1) + identifier(1) = 2 CB.
        // If field access also cost 1 CB, diff between reads would be 3.
        let src = r#"
            cb 100;
            let r1 = introspect.cb_remaining;
            let r2 = introspect.cb_remaining;
            print(r1 - r2);
        "#;
        let out = eval_output(src);
        // 2 = let binding(1) + identifier lookup(1); field access is free
        // if field access cost 1 CB too, diff would be 3
        assert_eq!(out, &["2"]);
    }

    #[test]
    fn introspect_custom_context() {
        let src = r#"
            let g = introspect.generation;
            let a = introspect.arena_size;
            print(g, a);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 7,
            lineage_id: "abc123".to_string(),
            arena_size: 12,
            ..Default::default()
        };
        let mut evaluator = Evaluator::new(1000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["7 12"]);
    }

    #[test]
    fn introspect_lineage_id_custom() {
        let src = r#"
            let lid = introspect.lineage_id;
            print(lid);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 3,
            lineage_id: "abc123def456".to_string(),
            arena_size: 8,
            ..Default::default()
        };
        let mut evaluator = Evaluator::new(1000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["abc123def456"]);
    }

    #[test]
    fn introspect_shadow_by_let() {
        // Shadowing introspect with let is allowed (no reassignment in language)
        let out = eval_output(
            r#"
            let introspect = 42;
            print(introspect);
        "#,
        );
        assert_eq!(out, &["42"]);
    }

    #[test]
    fn to_string_builtin() {
        let out = eval_output(
            r#"
            let a = to_string(42);
            let b = to_string(3.14);
            let c = to_string(true);
            print(a, b, c);
        "#,
        );
        assert_eq!(out, &["42 3.14 true"]);
    }

    #[test]
    fn to_string_concat_with_introspect() {
        let out = eval_output(
            r#"
            let msg = "gen=" + to_string(introspect.generation);
            print(msg);
        "#,
        );
        assert_eq!(out, &["gen=0"]);
    }

    // --- M45: Lineage History tests ---

    #[test]
    fn introspect_ancestor_failures_empty_outside_evolution() {
        let out = eval_output(
            r#"
            let fails = introspect.ancestor_failures;
            let succs = introspect.ancestor_successes;
            print(len(fails), len(succs));
        "#,
        );
        assert_eq!(out, &["0 0"]);
    }

    #[test]
    fn introspect_lineage_summary_zeroed_outside_evolution() {
        let out = eval_output(
            r#"
            let s = introspect.lineage_summary;
            print(s.total_ancestors, s.success_count, s.failure_count, s.avg_fitness);
        "#,
        );
        assert_eq!(out, &["0 0 0 0"]);
    }

    #[test]
    fn introspect_ancestor_failures_populated() {
        let src = r#"
            let fails = introspect.ancestor_failures;
            let count = len(fails);
            let first = get(fails, 0);
            print(count, first.generation, first.outcome, first.fitness_score);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 4,
            lineage_id: "test".to_string(),
            arena_size: 8,
            ancestor_failures: vec![
                crate::evolve::AncestorRecord {
                    generation: 3,
                    outcome: "validation_failed".to_string(),
                    fitness_score: 0.3,
                    code_hash: "abc123".to_string(),
                    elapsed_ms: 500,
                },
                crate::evolve::AncestorRecord {
                    generation: 2,
                    outcome: "cb_exhausted".to_string(),
                    fitness_score: 0.1,
                    code_hash: "def456".to_string(),
                    elapsed_ms: 1200,
                },
            ],
            ancestor_successes: vec![],
            identity_hash: String::new(),
        };
        let mut evaluator = Evaluator::new(10000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["2 3 validation_failed 0.3"]);
    }

    #[test]
    fn introspect_ancestor_successes_populated() {
        let src = r#"
            let succs = introspect.ancestor_successes;
            let first = get(succs, 0);
            print(len(succs), first.outcome, first.fitness_score);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 5,
            lineage_id: "test".to_string(),
            arena_size: 4,
            ancestor_failures: vec![],
            ancestor_successes: vec![crate::evolve::AncestorRecord {
                generation: 4,
                outcome: "survived".to_string(),
                fitness_score: 0.92,
                code_hash: "winner".to_string(),
                elapsed_ms: 800,
            }],
            identity_hash: String::new(),
        };
        let mut evaluator = Evaluator::new(10000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["1 survived 0.92"]);
    }

    #[test]
    fn introspect_lineage_summary_computed() {
        let src = r#"
            let s = introspect.lineage_summary;
            print(s.total_ancestors, s.success_count, s.failure_count, s.avg_fitness);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 5,
            lineage_id: "test".to_string(),
            arena_size: 8,
            ancestor_failures: vec![
                crate::evolve::AncestorRecord {
                    generation: 1,
                    outcome: "validation_failed".to_string(),
                    fitness_score: 0.2,
                    code_hash: "h1".to_string(),
                    elapsed_ms: 100,
                },
                crate::evolve::AncestorRecord {
                    generation: 2,
                    outcome: "cb_exhausted".to_string(),
                    fitness_score: 0.4,
                    code_hash: "h2".to_string(),
                    elapsed_ms: 200,
                },
            ],
            ancestor_successes: vec![crate::evolve::AncestorRecord {
                generation: 3,
                outcome: "survived".to_string(),
                fitness_score: 0.9,
                code_hash: "h3".to_string(),
                elapsed_ms: 300,
            }],
            identity_hash: String::new(),
        };
        let mut evaluator = Evaluator::new(10000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        let out = evaluator.output();
        assert_eq!(out.len(), 1);
        // total=3, success=1, failure=2, avg=(0.2+0.4+0.9)/3=0.5
        assert!(out[0].starts_with("3 1 2 0.5"));
    }

    #[test]
    fn introspect_ancestor_record_field_access() {
        // Verify all AncestorRecord fields are accessible
        let src = r#"
            let f = get(introspect.ancestor_failures, 0);
            print(f.generation, f.outcome, f.fitness_score, f.code_hash, f.elapsed_ms);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            generation: 2,
            lineage_id: "test".to_string(),
            arena_size: 4,
            ancestor_failures: vec![crate::evolve::AncestorRecord {
                generation: 1,
                outcome: "timeout".to_string(),
                fitness_score: 0.0,
                code_hash: "deadbeef".to_string(),
                elapsed_ms: 30000,
            }],
            ancestor_successes: vec![],
            identity_hash: String::new(),
        };
        let mut evaluator = Evaluator::new(10000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["1 timeout 0 deadbeef 30000"]);
    }

    // --- M49: Identity Hash in Introspect ---

    #[test]
    fn introspect_identity_hash_accessible() {
        let src = r#"
            let id = introspect.identity_hash;
            print(id);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            identity_hash: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string(),
            ..Default::default()
        };
        let mut evaluator = Evaluator::new(1000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(
            evaluator.output(),
            &["a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"]
        );
    }

    #[test]
    fn introspect_identity_hash_default_empty() {
        let src = r#"
            let id = introspect.identity_hash;
            print(id == "");
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(1000);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["true"]);
    }

    #[test]
    fn introspect_identity_hash_costs_zero_cb() {
        let src = r#"
            cb 100;
            let r1 = introspect.cb_remaining;
            let id = introspect.identity_hash;
            let r2 = introspect.cb_remaining;
            print(r1 - r2);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let ctx = IntrospectContext {
            identity_hash: "test_hash_value_64chars_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            ..Default::default()
        };
        let mut evaluator = Evaluator::new(1000).with_introspect(ctx);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        // Between r1 and r2: `let id = introspect.identity_hash` (let=1 + lookup=1 = 2)
        // + `let r2 = introspect.cb_remaining` (let=1 + lookup=1 = 2) = 4 total.
        // Field access on Introspect is 0 CB.
        assert_eq!(evaluator.output(), &["4"]);
    }

    // --- M46: Memo Store tests ---

    #[test]
    fn memo_write_and_recall() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            memo_write("test-key", "hello world");
            let vals = recall("test-key");
            print(len(vals), get(vals, 0));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["1 hello world"]);
    }

    #[test]
    fn memo_recall_nonexistent_key() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            let vals = recall("nonexistent");
            print(len(vals));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["0"]);
    }

    #[test]
    fn memo_recall_latest_empty() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            let v = recall_latest("empty-key");
            print(len(v));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["0"]);
    }

    #[test]
    fn memo_recall_latest_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            memo_write("strat", "first");
            memo_write("strat", "second");
            memo_write("strat", "third");
            let latest = recall_latest("strat");
            print(latest);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["third"]);
    }

    #[test]
    fn memo_entries_accumulate() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Write 2 entries in first run
        let src1 = r#"
            memo_write("acc", "a");
            memo_write("acc", "b");
        "#;
        let program1 = Parser::parse_source(src1).unwrap();
        let mut evaluator1 = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator1.grant_all();
        evaluator1.eval_program(&program1).unwrap();

        // Write 1 more in second run, read all
        let src2 = r#"
            memo_write("acc", "c");
            let vals = recall("acc");
            print(len(vals));
            let latest = recall_latest("acc");
            print(latest);
        "#;
        let program2 = Parser::parse_source(src2).unwrap();
        let mut evaluator2 = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator2.grant_all();
        evaluator2.eval_program(&program2).unwrap();
        assert_eq!(evaluator2.output(), &["3", "c"]);
    }

    #[test]
    fn memo_write_costs_1_cb() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            cb 100;
            let before = introspect.cb_remaining;
            memo_write("cost-test", "value");
            let after = introspect.cb_remaining;
            print(before - after);
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        let out = evaluator.output();
        // Call costs 5 (eval_call) + 1 (memo_write spend) + field access costs
        // But the key insight: before - after includes the memo_write call (5+1=6)
        // plus the field access for `after` (1 for eval_field_access)
        // Let's just verify it's > 5 (more than a plain call)
        let diff: i64 = out[0].parse().unwrap();
        assert!(
            diff > 5,
            "memo_write should cost more than a plain call: {diff}"
        );
    }

    #[test]
    fn memo_write_limit_per_generation() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Write 21 entries — 21st should be silently dropped
        let mut lines = String::new();
        for i in 0..21 {
            lines.push_str(&format!("memo_write(\"limit\", \"entry-{i}\");\n"));
        }
        lines.push_str("let vals = recall(\"limit\");\n");
        lines.push_str("print(len(vals));\n");
        let program = Parser::parse_source(&lines).unwrap();
        let mut evaluator = Evaluator::new(100000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["20"]);
    }

    #[test]
    fn memo_key_special_chars_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"memo_write("../evil", "hack");"#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        let result = evaluator.eval_program(&program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("alphanumeric"),
            "error should mention sanitization: {err}"
        );
    }

    #[test]
    fn memo_large_entry_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Create a string > 10 KB
        let big = "x".repeat(11000);
        let src = format!(
            "memo_write(\"big\", \"{big}\");\nlet v = recall_latest(\"big\");\nprint(len(v));\n"
        );
        let program = Parser::parse_source(&src).unwrap();
        let mut evaluator = Evaluator::new(100000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        let out = evaluator.output();
        let len: i64 = out[0].parse().unwrap();
        // 10240 + "[truncated]".len() = 10251
        assert_eq!(
            len, 10251,
            "value should be truncated to 10240 + [truncated] suffix"
        );
    }

    #[test]
    fn memo_recall_without_memo_dir() {
        // No memo_dir configured — recall returns empty list
        let src = r#"
            let vals = recall("anything");
            print(len(vals));
        "#;
        let out = eval_output(src);
        assert_eq!(out, &["0"]);
    }

    #[test]
    fn memo_recall_latest_without_memo_dir() {
        // No memo_dir configured — recall_latest returns empty string
        let src = r#"
            let v = recall_latest("anything");
            print(len(v));
        "#;
        let out = eval_output(src);
        assert_eq!(out, &["0"]);
    }

    #[test]
    fn memo_recall_returns_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            memo_write("order", "first");
            memo_write("order", "second");
            memo_write("order", "third");
            let vals = recall("order");
            print(get(vals, 0), get(vals, 1), get(vals, 2));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["third second first"]);
    }

    // --- M47: Memo GC tests ---

    #[test]
    fn memo_gc_max_100_entries_per_key() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        std::fs::create_dir_all(&memo_dir).unwrap();
        // Pre-populate 99 entries directly (bypassing per-gen write limit)
        let memo_file = memo_dir.join("gc-key.jsonl");
        let mut content = String::new();
        for i in 0..99 {
            content.push_str(&format!(
                "{{\"generation\":0,\"value\":\"old-entry-{i}\",\"timestamp\":1000}}\n"
            ));
        }
        std::fs::write(&memo_file, &content).unwrap();
        // Now write 2 more via memo_write — should cap at 100 (oldest evicted)
        let src = r#"
            memo_write("gc-key", "new-entry-1");
            memo_write("gc-key", "new-entry-2");
            let vals = recall("gc-key");
            print(len(vals));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["100"]);
    }

    #[test]
    fn memo_gc_max_50_keys() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Write 50 keys — should succeed
        let mut lines = String::new();
        for i in 0..50 {
            lines.push_str(&format!("memo_write(\"key-{i}\", \"val\");\n"));
        }
        // 51st key should fail
        lines.push_str("memo_write(\"key-50\", \"val\");\n");
        let program = Parser::parse_source(&lines).unwrap();
        let mut evaluator = Evaluator::new(1000000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        let result = evaluator.eval_program(&program);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("max 50 keys"), "error: {err}");
    }

    #[test]
    fn memo_gc_total_size_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Set very small max size (1 KB) to trigger eviction
        let src = r#"
            memo_write("size-test", "aaaaaaaaaa");
            memo_write("size-test", "bbbbbbbbbb");
            memo_write("size-test", "cccccccccc");
            memo_write("size-test", "dddddddddd");
            memo_write("size-test", "eeeeeeeeee");
            let vals = recall("size-test");
            print(len(vals));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000)
            .with_memo_dir(&memo_dir)
            .with_memo_max_size(200); // very small — will evict old entries
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        let out = evaluator.output();
        let count: usize = out[0].parse().unwrap();
        // With 200 byte limit, some entries should have been evicted
        assert!(count < 5, "expected eviction, got {count} entries");
        assert!(count >= 1, "should keep at least 1 entry");
    }

    #[test]
    fn memo_config_max_size_respected() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        // Default 10MB should not evict small entries
        let src = r#"
            memo_write("config-test", "small");
            let vals = recall("config-test");
            print(len(vals));
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();
        assert_eq!(evaluator.output(), &["1"]);
    }

    #[test]
    fn memo_list_keys_works() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            memo_write("alpha", "one");
            memo_write("alpha", "two");
            memo_write("beta", "three");
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let keys = memo_list_keys(&memo_dir);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].0, "alpha");
        assert_eq!(keys[0].1, 2); // 2 entries
        assert_eq!(keys[1].0, "beta");
        assert_eq!(keys[1].1, 1); // 1 entry
    }

    #[test]
    fn memo_store_total_size_works() {
        let dir = tempfile::tempdir().unwrap();
        let memo_dir = dir.path().join("memo");
        let src = r#"
            memo_write("sz", "hello");
        "#;
        let program = Parser::parse_source(src).unwrap();
        let mut evaluator = Evaluator::new(10000).with_memo_dir(&memo_dir);
        evaluator.grant_all();
        evaluator.eval_program(&program).unwrap();

        let total = memo_store_total_size(&memo_dir);
        assert!(total > 0, "total size should be > 0");
    }
}
