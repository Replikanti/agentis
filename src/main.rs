mod ast;
mod audit;
mod capabilities;
mod compiler;
mod config;
mod error;
mod evaluator;
mod io;
mod json;
mod lexer;
mod llm;
mod network;
mod parser;
mod pii;
mod refs;
mod snapshot;
mod storage;
mod trace;
mod typechecker;

use std::path::Path;
use std::process;

use error::AgentisError;
use evaluator::Evaluator;
use parser::Parser;
use refs::Refs;
use storage::ObjectStore;

const DEFAULT_BUDGET: u64 = 10000;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    let result = match args[1].as_str() {
        "init" => cmd_init(),
        "commit" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis commit <source_file>");
                process::exit(1);
            }
            cmd_commit(&args[2])
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis run <branch>");
                process::exit(1);
            }
            cmd_run(&args[2])
        }
        "go" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis go <source_file> [--trace] [--grant-pii]");
                process::exit(1);
            }
            let force_verbose = args.iter().any(|a| a == "--trace");
            let grant_pii = args.iter().any(|a| a == "--grant-pii");
            cmd_go(&args[2], force_verbose, grant_pii)
        }
        "doctor" => cmd_doctor(),
        "branch" => {
            if args.len() < 3 {
                cmd_list_branches()
            } else {
                cmd_create_branch(&args[2])
            }
        }
        "switch" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis switch <branch>");
                process::exit(1);
            }
            cmd_switch(&args[2])
        }
        "compile" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis compile <branch> [output.wasm]");
                process::exit(1);
            }
            let output = args.get(3).map(|s| s.as_str());
            cmd_compile(&args[2], output)
        }
        "sync" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis sync <host:port>");
                process::exit(1);
            }
            cmd_sync(&args[2])
        }
        "serve" => {
            let addr = args.get(2).map(|s| s.as_str()).unwrap_or("0.0.0.0:9461");
            cmd_serve(addr)
        }
        "audit" => cmd_audit(&args[2..]),
        "log" => {
            let branch = args.get(2).map(|s| s.as_str());
            cmd_log(branch)
        }
        "version" => {
            println!("agentis v0.1.0");
            Ok(())
        }
        other => {
            eprintln!("Unknown command: {other}");
            print_usage();
            process::exit(1);
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

fn print_usage() {
    eprintln!("Usage: agentis <command> [args]");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  init                 Initialize a new Agentis repository");
    eprintln!("  go <file> [--trace]  Commit and run in one step (the demo command)");
    eprintln!("  commit <file>        Parse source file, store AST, update current branch");
    eprintln!("  run <branch>         Execute code from a branch's root hash");
    eprintln!("  doctor               Check environment and configuration");
    eprintln!("  branch [name]        List branches or create a new one");
    eprintln!("  switch <branch>      Switch to a different branch");
    eprintln!("  compile <branch> [o] Compile branch to WASM binary");
    eprintln!("  sync <host:port>     Sync objects with a remote peer");
    eprintln!("  serve [addr:port]    Listen for incoming sync connections");
    eprintln!("  log [branch]         Show commit log for a branch");
    eprintln!("  audit [flags]        Show prompt audit log");
    eprintln!("  version              Show version");
}

fn agentis_root() -> std::path::PathBuf {
    Path::new(".agentis").to_path_buf()
}

fn ensure_initialized() -> Result<(ObjectStore, Refs), AgentisError> {
    let root = agentis_root();
    if !root.exists() {
        return Err(AgentisError::General(
            "Not an Agentis repository. Run 'agentis init' first.".to_string(),
        ));
    }
    Ok((ObjectStore::new(&root), Refs::new(&root)))
}

fn cmd_init() -> Result<(), AgentisError> {
    let root = agentis_root();
    if root.exists() {
        return Err(AgentisError::General(
            "Agentis repository already initialized.".to_string(),
        ));
    }
    ObjectStore::init(&root)?;
    let refs = Refs::new(&root);
    refs.init()?;

    // Create sandbox directory
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox)?;

    // Write default config with templates
    let config_path = root.join("config");
    std::fs::write(&config_path, DEFAULT_CONFIG)?;

    // Extract bundled examples
    let examples_dir = Path::new("examples");
    if !examples_dir.exists() {
        std::fs::create_dir_all(examples_dir)?;
        for (name, content) in BUNDLED_EXAMPLES {
            std::fs::write(examples_dir.join(name), content)?;
        }
        // Also copy data.txt to sandbox for pipeline example
        std::fs::write(sandbox.join("data.txt"), EXAMPLE_DATA)?;
        println!("Created examples/ directory with 6 programs.");
    }

    println!("Initialized empty Agentis repository with genesis branch.");
    println!();
    println!("  agentis go examples/fast-demo.ag    # try it now");
    Ok(())
}

const BUNDLED_EXAMPLES: &[(&str, &str)] = &[
    ("fast-demo.ag", include_str!("../examples/fast-demo.ag")),
    ("hello.ag", include_str!("../examples/hello.ag")),
    ("classify.ag", include_str!("../examples/classify.ag")),
    ("pipeline.ag", include_str!("../examples/pipeline.ag")),
    ("parallel.ag", include_str!("../examples/parallel.ag")),
    ("explore.ag", include_str!("../examples/explore.ag")),
    ("README.md", include_str!("../examples/README.md")),
];

const EXAMPLE_DATA: &str = include_str!("../examples/data.txt");

const DEFAULT_CONFIG: &str = "\
# Agentis Configuration
# Uncomment ONE LLM backend section below.

llm.backend = mock

# --- Claude CLI (flat-rate, recommended) ---
# llm.backend = cli
# llm.command = claude
# llm.args = -p --output-format text

# --- Ollama (local, free) ---
# llm.backend = cli
# llm.command = ollama
# llm.args = run llama3

# --- Anthropic API (per-token) ---
# llm.backend = http
# llm.endpoint = https://api.anthropic.com/v1/messages
# llm.model = claude-sonnet-4-20250514
# llm.api_key_env = ANTHROPIC_API_KEY

# --- Gemini CLI ---
# llm.backend = cli
# llm.command = gemini
# llm.args = -p

# --- xAI / Grok API ---
# llm.backend = http
# llm.endpoint = https://api.x.ai/v1/messages
# llm.model = grok-3
# llm.api_key_env = XAI_API_KEY

# Agent limits
# max_concurrent_agents = 16

# Trace (quiet = only LLM wait, normal = agent lifecycle, verbose = everything)
trace.level = normal
";

fn cmd_go(source_file: &str, force_verbose: bool, grant_pii: bool) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    // Commit
    let source = std::fs::read_to_string(source_file)?;
    let program = Parser::parse_source(&source)?;
    let tree_hash = store.save(&program)?;
    let (branch, commit_hash) = refs.commit(&tree_hash, &store)?;
    eprintln!("[{branch}] {}", &commit_hash[..12]);

    // Type check
    let type_errors = typechecker::check(&program);
    for err in &type_errors {
        eprintln!("warning: {err}");
    }

    // Run
    let cfg = config::Config::load(&agentis_root());
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&agentis_root(), &cfg);
    let trace_level = if force_verbose {
        trace::TraceLevel::Verbose
    } else {
        trace::TraceLevel::from_str(&cfg.get_or("trace.level", "normal"))
    };
    let tracer = trace::Tracer::new(trace_level);

    let audit_log = audit::AuditLog::open(&agentis_root());

    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer);
    if let Some(ref audit) = audit_log {
        evaluator = evaluator.with_audit(audit);
    }
    evaluator.grant_all();

    // PiiTransmit: grant only if --grant-pii flag or config says allow
    if grant_pii || cfg.get("pii_transmit").is_some_and(|v| v == "allow") {
        evaluator.grant(capabilities::CapKind::PiiTransmit);
    }
    match evaluator.eval_program(&program) {
        Ok(_) => {
            for line in evaluator.output() {
                println!("{line}");
            }
            for b in evaluator.explore_branches() {
                println!("  explore: created branch '{b}'");
            }
            Ok(())
        }
        Err(e) => Err(AgentisError::Eval(e)),
    }
}

fn cmd_commit(source_file: &str) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    let source = std::fs::read_to_string(source_file)?;
    let program = Parser::parse_source(&source)?;
    let tree_hash = store.save(&program)?;
    let (branch, commit_hash) = refs.commit(&tree_hash, &store)?;

    println!("[{branch}] {}", &commit_hash[..12]);
    Ok(())
}

fn cmd_run(branch: &str) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    let tree_hash = refs.resolve_tree(branch, &store)?
        .ok_or_else(|| AgentisError::General(format!("branch '{branch}' has no commits")))?;

    let program: ast::Program = store.load(&tree_hash)?;

    // Static type check (warnings only — does not block execution)
    let type_errors = typechecker::check(&program);
    for err in &type_errors {
        eprintln!("warning: {err}");
    }

    // Load config, LLM backend, I/O context, and tracer
    let cfg = config::Config::load(&agentis_root());
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&agentis_root(), &cfg);
    let trace_level = trace::TraceLevel::from_str(
        &cfg.get_or("trace.level", "normal"),
    );
    let tracer = trace::Tracer::new(trace_level);

    let audit_log = audit::AuditLog::open(&agentis_root());

    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer);
    if let Some(ref audit) = audit_log {
        evaluator = evaluator.with_audit(audit);
    }
    evaluator.grant_all();

    // PiiTransmit from config only (no CLI flag for `run`)
    if cfg.get("pii_transmit").is_some_and(|v| v == "allow") {
        evaluator.grant(capabilities::CapKind::PiiTransmit);
    }

    match evaluator.eval_program(&program) {
        Ok(_) => {
            for line in evaluator.output() {
                println!("{line}");
            }
            if !evaluator.explore_branches().is_empty() {
                for b in evaluator.explore_branches() {
                    println!("  explore: created branch '{b}'");
                }
            }
            Ok(())
        }
        Err(e) => Err(AgentisError::Eval(e)),
    }
}

fn cmd_doctor() -> Result<(), AgentisError> {
    let root = agentis_root();

    // Check .agentis/ exists
    if !root.exists() {
        println!("[!!] .agentis/ not found (run 'agentis init')");
        return Ok(());
    }
    println!("[ok] .agentis/ repository found");

    // Check config
    let cfg = config::Config::load(&root);
    let backend = cfg.get_or("llm.backend", "mock");
    println!("[ok] config loaded (llm.backend = {backend})");

    // Check LLM backend
    match backend.as_str() {
        "cli" => {
            let command = cfg.get_or("llm.command", "claude");
            match which_command(&command) {
                Some(path) => println!("[ok] {command} found in PATH ({path})"),
                None => println!("[!!] {command} NOT found in PATH"),
            }
        }
        "http" => {
            let endpoint = cfg.get_or("llm.endpoint", "(not set)");
            println!("[ok] HTTP endpoint: {endpoint}");
            let key_env = cfg.get_or("llm.api_key_env", "ANTHROPIC_API_KEY");
            if std::env::var(&key_env).is_ok() {
                println!("[ok] {key_env} environment variable is set");
            } else {
                println!("[!!] {key_env} environment variable NOT set");
            }
        }
        "mock" => {
            println!("[ok] using mock backend (no LLM calls)");
        }
        other => {
            println!("[!!] unknown backend: {other}");
        }
    }

    // Check sandbox
    let sandbox = root.join("sandbox");
    if sandbox.exists() {
        // Test writability
        let test_file = sandbox.join(".doctor_test");
        match std::fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_file);
                println!("[ok] .agentis/sandbox/ exists (writable)");
            }
            Err(_) => println!("[!!] .agentis/sandbox/ exists but NOT writable"),
        }
    } else {
        println!("[!!] .agentis/sandbox/ does not exist");
    }

    // Trace level
    let trace_level = cfg.get_or("trace.level", "normal");
    if trace_level == "quiet" {
        println!("[..] trace.level = quiet (consider 'normal' for debugging)");
    } else {
        println!("[ok] trace.level = {trace_level}");
    }

    Ok(())
}

/// Check if a command exists in PATH (like `which`).
fn which_command(cmd: &str) -> Option<String> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let full = Path::new(dir).join(cmd);
        if full.exists() {
            return Some(full.display().to_string());
        }
    }
    None
}

fn cmd_list_branches() -> Result<(), AgentisError> {
    let (_, refs) = ensure_initialized()?;
    let branches = refs.list_branches()?;
    for (name, is_current) in &branches {
        if *is_current {
            println!("* {name}");
        } else {
            println!("  {name}");
        }
    }
    Ok(())
}

fn cmd_create_branch(name: &str) -> Result<(), AgentisError> {
    let (_, refs) = ensure_initialized()?;
    refs.create_branch(name, None)?;
    println!("Created branch '{name}'.");
    Ok(())
}

fn cmd_switch(name: &str) -> Result<(), AgentisError> {
    let (_, refs) = ensure_initialized()?;
    refs.switch_branch(name)?;
    println!("Switched to branch '{name}'.");
    Ok(())
}

fn cmd_compile(branch: &str, output: Option<&str>) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    let tree_hash = refs.resolve_tree(branch, &store)?
        .ok_or_else(|| AgentisError::General(format!("branch '{branch}' has no commits")))?;

    let wasm = compiler::compile_from_store(&store, &tree_hash)?;

    let output_path = match output {
        Some(p) => p.to_string(),
        None => format!("{}.wasm", branch),
    };

    std::fs::write(&output_path, &wasm)?;
    println!("Compiled to {} ({} bytes)", output_path, wasm.len());
    Ok(())
}

fn cmd_sync(addr: &str) -> Result<(), AgentisError> {
    let (store, _) = ensure_initialized()?;
    let result = network::sync_push_pull(&store, addr)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    println!("Sync complete: sent {}, received {}", result.sent, result.received);
    Ok(())
}

fn cmd_serve(addr: &str) -> Result<(), AgentisError> {
    let (store, _) = ensure_initialized()?;
    println!("Listening on {addr}...");
    let result = network::sync_serve_once(&store, addr)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    println!("Sync complete: sent {}, received {}", result.sent, result.received);
    Ok(())
}

fn cmd_log(branch: Option<&str>) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    let branch_name = match branch {
        Some(b) => b.to_string(),
        None => refs.current_branch()?,
    };

    let log = refs.log(&branch_name, &store)?;
    if log.is_empty() {
        println!("No commits on branch '{branch_name}'.");
    } else {
        for commit in &log {
            let short_tree = if commit.tree_hash.len() >= 12 {
                &commit.tree_hash[..12]
            } else {
                &commit.tree_hash
            };
            println!("commit  tree:{short_tree}  ({branch_name})");
        }
    }
    Ok(())
}

fn cmd_audit(args: &[String]) -> Result<(), AgentisError> {
    let root = agentis_root();
    let audit_path = root.join("audit").join("prompts.jsonl");

    if !audit_path.exists() {
        eprintln!("No audit log found. Enable auditing by creating .agentis/audit/ directory.");
        eprintln!("  mkdir -p .agentis/audit");
        eprintln!("  (or use 'agentis init --secure')");
        return Ok(());
    }

    // Parse flags
    let mut last_n: usize = 50;
    let mut pii_only = false;
    let mut blocked_only = false;
    let mut agent_filter: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--last" => {
                i += 1;
                if i < args.len() {
                    last_n = args[i].parse().unwrap_or(50);
                }
            }
            "--pii-only" => pii_only = true,
            "--blocked" => blocked_only = true,
            "--agent" => {
                i += 1;
                if i < args.len() {
                    agent_filter = Some(args[i].clone());
                }
            }
            "--help" => {
                eprintln!("Usage: agentis audit [flags]");
                eprintln!();
                eprintln!("Flags:");
                eprintln!("  --last N        Show last N entries (default: 50)");
                eprintln!("  --pii-only      Only show entries with PII detected");
                eprintln!("  --agent <name>  Filter by agent name");
                eprintln!("  --blocked       Only show blocked entries (PiiTransmit denied)");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    let content = std::fs::read_to_string(&audit_path)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        println!("Audit log is empty.");
        return Ok(());
    }

    // Parse and filter
    let mut entries: Vec<AuditEntry> = Vec::new();
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(val) = json::parse(line) {
            let entry = AuditEntry::from_json(&val);

            // Apply filters
            if pii_only && entry.pii_scan == "clean" {
                continue;
            }
            if blocked_only && entry.pii_transmit_granted {
                continue;
            }
            if blocked_only && entry.pii_scan == "clean" {
                continue;
            }
            if let Some(ref agent) = agent_filter {
                if entry.agent != *agent {
                    continue;
                }
            }

            entries.push(entry);
        }
    }

    // Take last N
    let start = if entries.len() > last_n { entries.len() - last_n } else { 0 };
    let entries = &entries[start..];

    if entries.is_empty() {
        println!("No matching audit entries.");
        return Ok(());
    }

    // Print table header
    println!("{:<12} {:<16} {:<18} {:<10} {}",
        "TIME", "AGENT", "PII", "STATUS", "BACKEND");

    for entry in entries {
        let time_str = format_unix_time(entry.ts);
        let agent = if entry.agent.len() > 14 {
            format!("{}...", &entry.agent[..11])
        } else {
            entry.agent.clone()
        };
        let pii = if entry.pii_scan == "clean" {
            "clean".to_string()
        } else {
            entry.pii_types.join(",")
        };
        let pii_display = if pii.len() > 16 {
            format!("{}...", &pii[..13])
        } else {
            pii
        };
        let status = if entry.pii_scan == "clean" {
            "\u{2014}".to_string() // em-dash
        } else if entry.pii_transmit_granted {
            "GRANTED".to_string()
        } else {
            "BLOCKED".to_string()
        };
        let backend = if entry.backend.is_empty() && entry.model.is_empty() {
            "\u{2014}".to_string()
        } else if entry.model.is_empty() {
            entry.backend.clone()
        } else {
            format!("{}/{}", entry.backend, entry.model)
        };

        println!("{:<12} {:<16} {:<18} {:<10} {}",
            time_str, agent, pii_display, status, backend);
    }

    println!("\n({} entries shown)", entries.len());
    Ok(())
}

struct AuditEntry {
    ts: i64,
    agent: String,
    pii_scan: String,
    pii_types: Vec<String>,
    pii_transmit_granted: bool,
    backend: String,
    model: String,
}

impl AuditEntry {
    fn from_json(val: &json::JsonValue) -> Self {
        Self {
            ts: val.get("ts").and_then(|v| v.as_i64()).unwrap_or(0),
            agent: val.get("agent").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
            pii_scan: val.get("pii_scan").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
            pii_types: val.get("pii_types")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect())
                .unwrap_or_default(),
            pii_transmit_granted: val.get("pii_transmit_granted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            backend: val.get("backend").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            model: val.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        }
    }
}

fn format_unix_time(ts: i64) -> String {
    if ts == 0 {
        return "?".to_string();
    }
    // Convert to HH:MM:SS — simple manual conversion (no chrono dependency)
    let secs_in_day = ts % 86400;
    let hours = secs_in_day / 3600;
    let mins = (secs_in_day % 3600) / 60;
    let secs = secs_in_day % 60;
    format!("{hours:02}:{mins:02}:{secs:02}")
}
