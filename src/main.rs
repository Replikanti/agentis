mod arena;
mod ast;
mod audit;
mod capabilities;
mod compiler;
mod config;
mod error;
mod evaluator;
mod evolve;
mod fitness;
mod io;
mod json;
mod lexer;
mod llm;
mod mutation;
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
        "init" => {
            let secure = args.iter().any(|a| a == "--secure");
            cmd_init(secure)
        }
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
                eprintln!("Usage: agentis go <source_file> [--trace] [--grant-pii] [--fitness] [--weights W]");
                process::exit(1);
            }
            let force_verbose = args.iter().any(|a| a == "--trace");
            let grant_pii = args.iter().any(|a| a == "--grant-pii");
            let show_fitness = args.iter().any(|a| a == "--fitness");
            let weights = parse_flag_value(&args, "--weights");
            cmd_go(&args[2], force_verbose, grant_pii, show_fitness, weights.as_deref())
        }
        "repl" => cmd_repl(&args[2..]),
        "test" => cmd_test(&args[2..]),
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
        "snapshot" => cmd_snapshot(&args[2..]),
        "audit" => cmd_audit(&args[2..]),
        "arena" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis arena <files|dir> [--rounds N] [--top N] [--json] [--weights W]");
                process::exit(1);
            }
            cmd_arena(&args[2..])
        }
        "mutate" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis mutate <source_file> [flags]");
                eprintln!("       agentis mutate <source_file> --list-agents");
                process::exit(1);
            }
            cmd_mutate(&args[2], &args[3..])
        }
        "evolve" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis evolve <source_file> [flags]");
                eprintln!("  -g/--generations N  Number of generations (default: 5)");
                eprintln!("  -n/--population N   Population size (default: 4)");
                eprintln!("  --out <dir>         Output directory (default: evolved/)");
                eprintln!("  --agent <name>      Mutate only specific agent");
                eprintln!("  --weights W         Fitness weights (cb,val,exp)");
                eprintln!("  --budget-cap N      Max total CB across all evaluations");
                eprintln!("  --stop-on-stall N   Stop if no improvement for N generations");
                eprintln!("  --show-lineage      Show best agent's lineage");
                eprintln!("  --dry-run           Estimate cost without running");
                process::exit(1);
            }
            cmd_evolve(&args[2], &args[3..])
        }
        "lineage" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis lineage <source_file>");
                process::exit(1);
            }
            cmd_lineage(&args[2])
        }
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
    eprintln!("  init [--secure]       Initialize a new Agentis repository");
    eprintln!("  go <file> [flags]    Commit and run (--trace --grant-pii --fitness --weights W)");
    eprintln!("  repl [--resume <h>]  Interactive evaluator (REPL)");
    eprintln!("  test <files|dir>     Run tests (validate/explore outcomes)");
    eprintln!("  commit <file>        Parse source file, store AST, update current branch");
    eprintln!("  run <branch>         Execute code from a branch's root hash");
    eprintln!("  doctor               Check environment and configuration");
    eprintln!("  branch [name]        List branches or create a new one");
    eprintln!("  switch <branch>      Switch to a different branch");
    eprintln!("  compile <branch> [o] Compile branch to WASM binary");
    eprintln!("  sync <host:port>     Sync objects with a remote peer");
    eprintln!("  serve [addr:port]    Listen for incoming sync connections");
    eprintln!("  log [branch]         Show commit log for a branch");
    eprintln!("  snapshot list|show   List or inspect snapshots");
    eprintln!("  arena <files|dir>    Rank variants by fitness (--rounds --top --json)");
    eprintln!("  mutate <file> [flags] Generate mutated agent variants");
    eprintln!("  evolve <file> [flags] Evolutionary loop (-g N -n N --out dir)");
    eprintln!("  lineage <file>       Trace variant ancestry back to seed");
    eprintln!("  audit [flags]        Show prompt audit log");
    eprintln!("  version              Show version");
}

fn agentis_root() -> std::path::PathBuf {
    Path::new(".agentis").to_path_buf()
}

/// Parse a flag like `--weights 0.3,0.5,0.2` from args, returning the value after the flag.
fn parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
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

fn cmd_init(secure: bool) -> Result<(), AgentisError> {
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

    // Write config (secure or default)
    let config_path = root.join("config");
    if secure {
        std::fs::write(&config_path, SECURE_CONFIG)?;
        // Create audit directory (enables audit logging)
        std::fs::create_dir_all(root.join("audit"))?;
    } else {
        std::fs::write(&config_path, DEFAULT_CONFIG)?;
    }

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

    if secure {
        println!("Initialized secure Agentis repository with genesis branch.");
        println!("  PII guard:  ON (PiiTransmit denied by default)");
        println!("  Audit log:  ON (.agentis/audit/)");
        println!("  LLM:        mock (configure in .agentis/config)");
    } else {
        println!("Initialized empty Agentis repository with genesis branch.");
        println!();
        println!("  agentis go examples/fast-demo.ag    # try it now");
    }
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

const SECURE_CONFIG: &str = "\
# Agentis Configuration (--secure)
# Security-first defaults for production use.

llm.backend = mock

# --- Claude CLI (flat-rate, recommended) ---
# llm.backend = cli
# llm.command = claude
# llm.args = -p --output-format text

# --- Ollama (local, free — recommended for sensitive data) ---
# llm.backend = cli
# llm.command = ollama
# llm.args = run llama3

# --- Anthropic API (per-token) ---
# llm.backend = http
# llm.endpoint = https://api.anthropic.com/v1/messages
# llm.model = claude-sonnet-4-20250514
# llm.api_key_env = ANTHROPIC_API_KEY

# PII Protection (Phase 5: Data Guardians)
# PiiTransmit is DENIED by default. To allow PII in prompts:
# pii_transmit = allow
pii_transmit = deny

# Audit logging (enabled — all prompts are logged)
audit = on

# Agent limits
# max_concurrent_agents = 16

# Trace (quiet = only LLM wait, normal = agent lifecycle, verbose = everything)
trace.level = normal
";

fn cmd_go(source_file: &str, force_verbose: bool, grant_pii: bool, show_fitness: bool, weights_str: Option<&str>) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;

    // Parse fitness weights if provided
    let fitness_weights = match weights_str {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("--weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };

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
        .with_snapshot_registry(&agentis_root())
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
            if show_fitness {
                let report = evaluator.fitness_report();
                eprintln!();
                eprint!("{}", report.display(&fitness_weights));
                // Append to fitness registry
                let entry = report.to_jsonl(&commit_hash, &fitness_weights);
                if let Err(e) = fitness::append_to_registry(&agentis_root(), &entry) {
                    eprintln!("warning: could not write fitness registry: {e}");
                }
            }
            Ok(())
        }
        Err(e) => {
            if show_fitness {
                let mut report = evaluator.fitness_report();
                report.error = true;
                eprintln!();
                eprint!("{}", report.display(&fitness_weights));
                let entry = report.to_jsonl(&commit_hash, &fitness_weights);
                let _ = fitness::append_to_registry(&agentis_root(), &entry);
            }
            Err(AgentisError::Eval(e))
        }
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
        .with_snapshot_registry(&agentis_root())
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

fn cmd_test(args: &[String]) -> Result<(), AgentisError> {
    if args.is_empty() || args[0] == "--help" {
        eprintln!("Usage: agentis test <files|dir> [--fail-fast] [--verbose]");
        return Ok(());
    }

    let fail_fast = args.iter().any(|a| a == "--fail-fast");
    let verbose = args.iter().any(|a| a == "--verbose");

    // Collect file paths (expand directories)
    let mut files = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        let path = std::path::Path::new(arg);
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                let mut dir_files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().extension().is_some_and(|ext| ext == "ag")
                    })
                    .map(|e| e.path().to_string_lossy().to_string())
                    .collect();
                dir_files.sort();
                files.extend(dir_files);
            }
        } else {
            files.push(arg.clone());
        }
    }

    if files.is_empty() {
        eprintln!("No .ag files found.");
        return Ok(());
    }

    let (store, refs) = ensure_initialized()?;
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&root, &cfg);
    let trace_level = if verbose {
        trace::TraceLevel::Verbose
    } else {
        trace::TraceLevel::Quiet
    };
    let tracer = trace::Tracer::new(trace_level);

    let mut total_passed = 0usize;
    let mut total_failed = 0usize;
    let mut any_file_failed = false;

    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file}");
                eprintln!("  ERROR: {e}");
                total_failed += 1;
                any_file_failed = true;
                if fail_fast { break; }
                continue;
            }
        };

        let program = match Parser::parse_source(&source) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{file}");
                eprintln!("  PARSE ERROR: {e}");
                total_failed += 1;
                any_file_failed = true;
                if fail_fast { break; }
                continue;
            }
        };

        // Commit (so VCS-dependent features work)
        let _ = store.save(&program).ok();

        let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
        let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
            .with_test_mode()
            .with_vcs(&store, &refs)
            .with_persistence(&store)
            .with_snapshot_registry(&root)
            .with_llm(llm_backend.as_ref())
            .with_io(&io_ctx)
            .with_max_agents(max_agents)
            .with_tracer(&tracer);
        evaluator.grant_all();

        if cfg.get("pii_transmit").is_some_and(|v| v == "allow") {
            evaluator.grant(capabilities::CapKind::PiiTransmit);
        }

        // Run the program in test mode
        let run_error = evaluator.eval_program(&program).err();

        let outcomes = evaluator.test_outcomes();
        let file_has_tests = !outcomes.is_empty();

        println!("{file}");

        if !file_has_tests && run_error.is_none() {
            println!("  (no explore/validate blocks)");
        }

        // Report per-test outcomes
        for outcome in outcomes {
            let status = if outcome.passed { "PASS" } else { "FAIL" };
            let kind_label = match outcome.kind {
                evaluator::TestKind::Explore => format!("explore \"{}\"", outcome.name),
                evaluator::TestKind::Validate => outcome.name.clone(),
            };
            let dots = 40usize.saturating_sub(kind_label.len() + 2);
            let dot_str: String = std::iter::repeat('.').take(dots).collect();
            println!("  {kind_label} {dot_str} {status}");

            if !outcome.passed {
                if let Some(ref detail) = outcome.detail {
                    if verbose {
                        println!("    {detail}");
                    }
                }
            }

            if outcome.passed {
                total_passed += 1;
            } else {
                total_failed += 1;
                any_file_failed = true;
            }
        }

        // Report fatal errors (not explore/validate failures)
        if let Some(ref e) = run_error {
            println!("  ERROR: {e}");
            total_failed += 1;
            any_file_failed = true;
        }

        if any_file_failed && fail_fast {
            break;
        }
    }

    println!();
    let total = total_passed + total_failed;
    println!("Results: {} passed, {} failed, {} total", total_passed, total_failed, total);

    if any_file_failed {
        process::exit(1);
    }
    Ok(())
}

fn cmd_repl(args: &[String]) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;
    let root = agentis_root();

    let cfg = config::Config::load(&root);
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&root, &cfg);
    let trace_level = trace::TraceLevel::from_str(&cfg.get_or("trace.level", "normal"));
    let tracer = trace::Tracer::new(trace_level);
    let audit_log = audit::AuditLog::open(&root);

    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_snapshot_registry(&root)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer);
    if let Some(ref audit) = audit_log {
        evaluator = evaluator.with_audit(audit);
    }
    evaluator.grant_all();

    if cfg.get("pii_transmit").is_some_and(|v| v == "allow") {
        evaluator.grant(capabilities::CapKind::PiiTransmit);
    }

    // Load current branch program (register functions/types/agents)
    let branch_name = refs.current_branch().unwrap_or_else(|_| "genesis".to_string());
    if let Ok(Some(tree_hash)) = refs.resolve_tree(&branch_name, &store) {
        if let Ok(program) = store.load::<ast::Program>(&tree_hash) {
            for decl in &program.declarations {
                match decl {
                    ast::Declaration::Function(f) => {
                        let _ = evaluator.eval_repl_declaration(
                            &ast::Declaration::Function(f.clone()),
                        );
                    }
                    ast::Declaration::Agent(a) => {
                        let _ = evaluator.eval_repl_declaration(
                            &ast::Declaration::Agent(a.clone()),
                        );
                    }
                    ast::Declaration::Type(t) => {
                        let _ = evaluator.eval_repl_declaration(
                            &ast::Declaration::Type(t.clone()),
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    // --resume <hash>: restore snapshot with CB penalty
    let resume_hash = args.windows(2).find_map(|w| {
        if w[0] == "--resume" { Some(w[1].clone()) } else { None }
    });
    if let Some(ref prefix) = resume_hash {
        let full_hash = resolve_snapshot_hash(&root, prefix)?;
        let mgr = snapshot::SnapshotManager::new(&store).with_registry(&root);
        let snap = mgr.load(&full_hash)
            .map_err(|e| AgentisError::General(format!("{e}")))?;
        evaluator.restore_snapshot_with_penalty(&snap);
        eprintln!("Restored snapshot {}", &full_hash[..12]);
        eprintln!("  Budget: {} (after 30% resurrection tax)", evaluator.budget_remaining());
        eprintln!("  Output: {} lines", evaluator.output().len());
    }

    eprintln!("agentis repl — type .help for commands, .exit to quit");

    let stdin = std::io::stdin();
    let mut input_buf = String::new();
    let initial_budget = DEFAULT_BUDGET;
    let mut output_shown = evaluator.output().len();

    loop {
        // Prompt
        if input_buf.is_empty() {
            eprint!("agentis> ");
        } else {
            eprint!("   ...> ");
        }

        let mut line = String::new();
        if stdin.read_line(&mut line).unwrap_or(0) == 0 {
            // EOF
            break;
        }

        // Handle dot-commands (only on fresh input, not continuations)
        let trimmed = line.trim();
        if input_buf.is_empty() && trimmed.starts_with('.') {
            match trimmed {
                ".exit" | ".quit" => break,
                ".budget" => {
                    eprintln!("CB: {}/{}", evaluator.budget_remaining(), initial_budget);
                }
                ".snapshot" => {
                    let snap = evaluator.capture_snapshot();
                    let mgr_result = snapshot::SnapshotManager::new(&store)
                        .with_registry(&root)
                        .save(&snap);
                    match mgr_result {
                        Ok(hash) => eprintln!("Snapshot saved: {}", &hash[..12]),
                        Err(e) => eprintln!("Snapshot error: {e}"),
                    }
                }
                ".output" => {
                    let output = evaluator.output();
                    if output.is_empty() {
                        eprintln!("(no output)");
                    } else {
                        for line in output {
                            println!("{line}");
                        }
                    }
                }
                ".help" => {
                    eprintln!("  .exit      Quit REPL");
                    eprintln!("  .budget    Show remaining CB / initial budget");
                    eprintln!("  .snapshot  Manually save snapshot");
                    eprintln!("  .output    Show accumulated output buffer");
                    eprintln!("  .help      Show this help");
                }
                other => {
                    eprintln!("Unknown command: {other}");
                    eprintln!("Type .help for available commands.");
                }
            }
            continue;
        }

        input_buf.push_str(&line);

        // Multi-line detection: if braces aren't balanced, continue reading
        let open = input_buf.matches('{').count();
        let close = input_buf.matches('}').count();
        if open > close {
            continue;
        }

        let input = input_buf.trim().to_string();
        input_buf.clear();

        if input.is_empty() {
            continue;
        }

        // Parse and evaluate
        match parser::Parser::parse_repl_input(&input) {
            Ok(decl) => {
                // For let statements, we want to show the assigned value
                let is_let = matches!(&decl, ast::Declaration::Statement(ast::Statement::Let(_)));

                match evaluator.eval_repl_declaration(&decl) {
                    Ok(val) => {
                        // Flush new print() output
                        let current_output = evaluator.output();
                        for line in &current_output[output_shown..] {
                            println!("{line}");
                        }
                        output_shown = current_output.len();

                        if is_let {
                            if let ast::Declaration::Statement(ast::Statement::Let(l)) = &decl {
                                if let Some(v) = evaluator.lookup_var(&l.name) {
                                    println!("{v}");
                                }
                            }
                        } else if val != evaluator::Value::Void {
                            println!("{val}");
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("parse error: {e}");
            }
        }
    }

    Ok(())
}

fn cmd_snapshot(args: &[String]) -> Result<(), AgentisError> {
    let (store, _refs) = ensure_initialized()?;
    let root = agentis_root();

    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("--help");

    match subcmd {
        "list" => {
            let mgr = snapshot::SnapshotManager::new(&store).with_registry(&root);
            let snapshots = mgr.list_all();
            if snapshots.is_empty() {
                println!("No snapshots found.");
                return Ok(());
            }
            println!(
                "{:<14} {:<12} {:<9} {}",
                "HASH", "CB", "OUTPUT", "SCOPES"
            );
            for info in &snapshots {
                let short_hash = if info.hash.len() >= 12 {
                    &info.hash[..12]
                } else {
                    &info.hash
                };
                let output_desc = if info.output_lines == 1 {
                    "1 line".to_string()
                } else {
                    format!("{} lines", info.output_lines)
                };
                println!(
                    "{:<14} {:<12} {:<9} {}",
                    short_hash,
                    format!("{}", info.budget_remaining),
                    output_desc,
                    info.scope_count,
                );
            }
        }
        "show" => {
            let hash_arg = args.get(1).ok_or_else(|| {
                AgentisError::General("Usage: agentis snapshot show <hash>".to_string())
            })?;
            // Support prefix matching
            let full_hash = resolve_snapshot_hash(&root, hash_arg)?;
            let snap = snapshot::SnapshotManager::new(&store)
                .with_registry(&root)
                .load(&full_hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            println!("Snapshot: {full_hash}");
            println!("Budget:   {}", snap.budget_remaining);
            println!("Output:   {} lines", snap.output.len());
            println!("Scopes:   {}", snap.scopes.len());

            if !snap.output.is_empty() {
                println!();
                println!("--- Output ---");
                for line in &snap.output {
                    println!("  {line}");
                }
            }

            for (i, scope) in snap.scopes.iter().enumerate() {
                println!();
                println!("--- Scope {} ({} bindings) ---", i, scope.len());
                let mut keys: Vec<&String> = scope.keys().collect();
                keys.sort();
                for key in keys {
                    let val = &scope[key];
                    println!("  {key} = {val}");
                }
            }
        }
        "--help" | "help" | _ => {
            eprintln!("Usage: agentis snapshot <command>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  list              List all snapshots");
            eprintln!("  show <hash>       Show snapshot details");
        }
    }

    Ok(())
}

/// Resolve a possibly-abbreviated snapshot hash to its full hash.
fn resolve_snapshot_hash(agentis_root: &std::path::Path, prefix: &str) -> Result<String, AgentisError> {
    let hashes = snapshot::load_registry(agentis_root);
    let matches: Vec<&String> = hashes.iter().filter(|h| h.starts_with(prefix)).collect();
    match matches.len() {
        0 => Err(AgentisError::General(format!(
            "no snapshot matching '{prefix}'"
        ))),
        1 => Ok(matches[0].clone()),
        _ => Err(AgentisError::General(format!(
            "ambiguous prefix '{prefix}': matches {} snapshots",
            matches.len()
        ))),
    }
}

fn cmd_mutate(source_file: &str, args: &[String]) -> Result<(), AgentisError> {
    let source = std::fs::read_to_string(source_file)?;

    // --list-agents: just print agent names and instructions, then exit
    if args.iter().any(|a| a == "--list-agents") {
        let agents = mutation::extract_agents(&source)
            .map_err(|e| AgentisError::General(e))?;
        if agents.is_empty() {
            println!("No agents with prompt instructions found.");
        } else {
            for a in &agents {
                println!("  {} — \"{}\"", a.name, a.instruction);
            }
        }
        return Ok(());
    }

    // Parse flags
    let count: usize = parse_flag_value(args, "--count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let out_dir = parse_flag_value(args, "--out");
    let agent_filter = parse_flag_value(args, "--agent");
    let custom_template = parse_flag_value(args, "--mutate-prompt");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    // Load LLM backend from config
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;

    if dry_run {
        // Dry-run: show what would be generated without writing files
        let agents = mutation::extract_agents(&source)
            .map_err(|e| AgentisError::General(e))?;
        if agents.is_empty() {
            return Err(AgentisError::General(
                "no agents with prompt instructions found in source".to_string(),
            ));
        }

        let eligible: Vec<&mutation::AgentInfo> = match agent_filter.as_deref() {
            Some(name) => {
                let filtered: Vec<_> = agents.iter().filter(|a| a.name == name).collect();
                if filtered.is_empty() {
                    return Err(AgentisError::General(format!(
                        "agent '{}' not found. Available: {}",
                        name,
                        agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                    )));
                }
                filtered
            }
            None => agents.iter().collect(),
        };

        let is_mock = llm_backend.name() == "mock";
        for i in 0..count {
            let agent = eligible[i % eligible.len()];
            let new_instruction = if is_mock {
                mutation::mock_mutate(&agent.instruction, i)
            } else {
                mutation::llm_mutate(&agent.instruction, llm_backend.as_ref(), custom_template.as_deref())
                    .map_err(|e| AgentisError::General(e))?
            };
            println!("{}", mutation::format_dry_run(i, count, &agent.name, &agent.instruction, &new_instruction));
            if i + 1 < count {
                println!();
            }
        }
        return Ok(());
    }

    // Derive base name from source file (without .ag extension)
    let base_name = std::path::Path::new(source_file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "variant".to_string());

    let variants = mutation::generate_variants(
        &source,
        &base_name,
        count,
        agent_filter.as_deref(),
        llm_backend.as_ref(),
        custom_template.as_deref(),
    ).map_err(|e| AgentisError::General(e))?;

    // Determine output directory
    let output_dir = match out_dir {
        Some(ref dir) => std::path::PathBuf::from(dir),
        None => std::path::Path::new(source_file)
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf(),
    };

    // Create output directory if needed
    if !output_dir.exists() {
        std::fs::create_dir_all(&output_dir)?;
    }

    for variant in &variants {
        let path = output_dir.join(&variant.filename);
        std::fs::write(&path, &variant.source)?;
        println!(
            "  {} (mutated: {})",
            path.display(),
            variant.mutated_agents.join(", ")
        );
    }

    println!("\nGenerated {} variant(s).", variants.len());
    Ok(())
}

/// Parse a flag value checking two names (e.g., "-g" and "--generations").
fn parse_flag_value2(args: &[String], short: &str, long: &str) -> Option<String> {
    parse_flag_value(args, short).or_else(|| parse_flag_value(args, long))
}

fn cmd_evolve(source_file: &str, args: &[String]) -> Result<(), AgentisError> {
    let generations: usize = parse_flag_value2(args, "-g", "--generations")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let population: usize = parse_flag_value2(args, "-n", "--population")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let out_dir = parse_flag_value(args, "--out")
        .unwrap_or_else(|| "evolved".to_string());
    let agent_filter = parse_flag_value(args, "--agent");
    let custom_template = parse_flag_value(args, "--mutate-prompt");
    let weights_str = parse_flag_value(args, "--weights");
    let budget_cap: Option<u64> = parse_flag_value(args, "--budget-cap")
        .and_then(|s| s.parse().ok());
    let stop_on_stall: Option<usize> = parse_flag_value(args, "--stop-on-stall")
        .and_then(|s| s.parse().ok());
    let show_lineage = args.iter().any(|a| a == "--show-lineage");
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let fitness_weights = match weights_str.as_deref() {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("--weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };

    // Read seed source
    let seed_source = std::fs::read_to_string(source_file)?;
    let seed_hash = evolve::hash_source(&seed_source);

    // Derive base name
    let base_name = std::path::Path::new(source_file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "variant".to_string());

    // Load LLM backend
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;

    // Dry-run mode
    if dry_run {
        let agents = mutation::extract_agents(&seed_source)
            .map_err(|e| AgentisError::General(e))?;
        let prompt_count = agents.len().max(1); // rough estimate: at least 1 prompt per agent
        let avg_instruction_len = if agents.is_empty() { 30 } else {
            agents.iter().map(|a| a.instruction.len()).sum::<usize>() / agents.len()
        };
        let tps = cfg.get("ollama_tokens_per_second")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(30.0);
        print!("{}", evolve::format_dry_run(
            generations, population, prompt_count, llm_backend.name(), avg_instruction_len, tps,
        ));
        return Ok(());
    }

    // Initialize evaluator dependencies
    let (store, refs) = ensure_initialized()?;
    let io_ctx = io::IoContext::new(&root, &cfg);
    let tracer = trace::Tracer::new(trace::TraceLevel::Quiet);
    let audit_log = audit::AuditLog::open(&root);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let grant_pii = cfg.get("pii_transmit").is_some_and(|v| v == "allow");

    // Create output directory
    let out_path = std::path::PathBuf::from(&out_dir);
    std::fs::create_dir_all(&out_path)?;

    // Fitness storage directory
    let fitness_dir = root.join("fitness");

    eprintln!("Evolution: {}", source_file);
    eprintln!("  Population: {}, Generations: {}", population, generations);
    eprintln!();

    // Generation 0: seed
    let mut parents: Vec<(String, String)> = vec![(seed_source.clone(), seed_hash.clone())];
    let mut best_ever_score: f64 = 0.0;
    let mut best_ever_source = seed_source.clone();
    let mut best_ever_hash = seed_hash.clone();
    let mut stall_count: usize = 0;
    let mut cumulative_cb: u64 = 0;
    let mut gen_results: Vec<evolve::GenResult> = Vec::new();
    let mut first_gen_avg_prompts: f64 = 0.0;
    let mut cb_only_warned = false;

    for g in 1..=generations {
        // Generate variants from parents
        let mock_offset = (g - 1) * population;
        let tracked_variants = evolve::generate_tracked_variants(
            &parents,
            population,
            g,
            &base_name,
            agent_filter.as_deref(),
            llm_backend.as_ref(),
            custom_template.as_deref(),
            mock_offset,
        ).map_err(|e| AgentisError::General(e))?;

        // Write variant files to temp for arena evaluation
        let gen_dir = out_path.join(format!("g{g:02}"));
        std::fs::create_dir_all(&gen_dir)?;

        for v in &tracked_variants {
            let path = gen_dir.join(&v.filename);
            std::fs::write(&path, &v.source)?;
        }

        // Arena: evaluate each variant
        let mut scored: Vec<(evolve::TrackedVariant, arena::ArenaEntry)> = Vec::new();
        for v in &tracked_variants {
            let path = gen_dir.join(&v.filename);
            let entry = run_arena_variant(
                &path.to_string_lossy(),
                &store, &refs, &root, &cfg, llm_backend.as_ref(),
                &io_ctx, &tracer, audit_log.as_ref(), max_agents,
                grant_pii, &fitness_weights,
            );

            // Track CB usage
            if entry.error.is_none() {
                let cb_spent = ((1.0 - entry.cb_eff) * DEFAULT_BUDGET as f64) as u64;
                cumulative_cb += cb_spent;
            }

            scored.push((v.clone(), entry));
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal));

        // Compute generation stats
        let successful: Vec<&arena::ArenaEntry> = scored.iter()
            .map(|(_, e)| e)
            .filter(|e| e.error.is_none())
            .collect();
        let gen_best = scored.first().map(|(_, e)| e.score).unwrap_or(0.0);
        let gen_avg = if successful.is_empty() { 0.0 } else {
            successful.iter().map(|e| e.score).sum::<f64>() / successful.len() as f64
        };
        let gen_avg_prompts = if successful.is_empty() { 0.0 } else {
            successful.iter().map(|e| e.prompt_count as f64).sum::<f64>() / successful.len() as f64
        };

        if g == 1 {
            first_gen_avg_prompts = gen_avg_prompts;
        }

        // CB-only warning (once)
        if !cb_only_warned {
            let all_cb_only = scored.iter().all(|(_, e)| e.val_rate >= 1.0 && e.exp_rate >= 1.0 && e.error.is_none());
            if all_cb_only && !successful.is_empty() {
                eprintln!("  Warning: No validate/explore blocks — fitness = CB efficiency only.");
                cb_only_warned = true;
            }
        }

        // Print generation summary
        eprintln!("Gen {:>2}: best={:.3}  avg={:.3}  prompts={:.1}  ({} variants)",
            g, gen_best, gen_avg, gen_avg_prompts, tracked_variants.len());

        // Save generation best
        let best_variant = &scored[0].0;
        let best_filename = format!("g{g:02}-best.ag");
        std::fs::write(out_path.join(&best_filename), &best_variant.source)?;

        // Record lineage
        evolve::write_generation_jsonl(&fitness_dir, g, &scored, &fitness_weights)
            .map_err(|e| AgentisError::General(format!("failed to write lineage: {e}")))?;

        // Track generation result
        gen_results.push(evolve::GenResult {
            generation: g,
            best_score: gen_best,
            avg_score: gen_avg,
            avg_prompts: gen_avg_prompts,
            variant_count: tracked_variants.len(),
            best_source: best_variant.source.clone(),
            best_hash: best_variant.source_hash.clone(),
        });

        // Update best-ever
        if gen_best > best_ever_score {
            best_ever_score = gen_best;
            best_ever_source = best_variant.source.clone();
            best_ever_hash = best_variant.source_hash.clone();
            stall_count = 0;
        } else {
            stall_count += 1;
        }

        // Convergence warning
        if stall_count >= 3 && (stop_on_stall.is_none() || stall_count < stop_on_stall.unwrap()) {
            eprintln!("  Warning: Evolution stalled at generation {} (score: {:.3})", g, best_ever_score);
        }

        // Stop-on-stall
        if let Some(stall_limit) = stop_on_stall {
            if stall_count >= stall_limit {
                eprintln!("\nStopped: no improvement for {} generations (score: {:.3})", stall_limit, best_ever_score);
                break;
            }
        }

        // Budget cap
        if let Some(cap) = budget_cap {
            if cumulative_cb >= cap {
                eprintln!("\nBudget cap reached at generation {} ({}/{} CB spent)", g, cumulative_cb, cap);
                break;
            }
        }

        // Select top K = N/2 as parents for next generation
        let k = (population / 2).max(1);
        parents = scored.iter()
            .take(k)
            .map(|(v, _)| (v.source.clone(), v.source_hash.clone()))
            .collect();
    }

    // Write best-of-run
    let best_filename = format!("{}-best.ag", base_name);
    let best_path = out_path.join(&best_filename);
    std::fs::write(&best_path, &best_ever_source)?;

    eprintln!();
    eprintln!("Best agent: {} (score: {:.3})", best_path.display(), best_ever_score);

    // Show lineage if requested
    if show_lineage {
        let lineage = evolve::load_lineage(&fitness_dir);
        let chain = evolve::trace_lineage(&lineage, &best_ever_hash, source_file);
        if chain.len() > 1 {
            eprintln!("  Lineage: {}", evolve::format_lineage(&chain));
        }
    }

    // Efficiency summary
    if !gen_results.is_empty() && first_gen_avg_prompts > 0.0 {
        let last_avg_prompts = gen_results.last().unwrap().avg_prompts;
        if last_avg_prompts != first_gen_avg_prompts {
            let pct = ((last_avg_prompts - first_gen_avg_prompts) / first_gen_avg_prompts) * 100.0;
            if pct < 0.0 {
                eprintln!("  Efficiency: prompt calls {:.0}% ({:.1} → {:.1} avg)",
                    pct, first_gen_avg_prompts, last_avg_prompts);
            } else {
                eprintln!("  Efficiency: prompt calls +{:.0}% ({:.1} → {:.1} avg)",
                    pct, first_gen_avg_prompts, last_avg_prompts);
            }
        }
    }

    Ok(())
}

fn cmd_lineage(source_file: &str) -> Result<(), AgentisError> {
    let source = std::fs::read_to_string(source_file)?;
    let source_hash = evolve::hash_source(&source);

    let root = agentis_root();
    let fitness_dir = root.join("fitness");

    if !fitness_dir.exists() {
        return Err(AgentisError::General(
            "No fitness data found. Run 'agentis evolve' first.".to_string(),
        ));
    }

    let lineage = evolve::load_lineage(&fitness_dir);

    if lineage.is_empty() {
        return Err(AgentisError::General(
            "No lineage data found in .agentis/fitness/".to_string(),
        ));
    }

    // Get seed name from the file
    let seed_name = std::path::Path::new(source_file)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "seed".to_string());

    let chain = evolve::trace_lineage(&lineage, &source_hash, &seed_name);

    if chain.is_empty() {
        eprintln!("Source hash {} not found in lineage data.", &source_hash[..12]);
        return Ok(());
    }

    println!("{}", evolve::format_lineage(&chain));
    Ok(())
}

fn cmd_arena(args: &[String]) -> Result<(), AgentisError> {
    // Parse flags
    let rounds: usize = parse_flag_value(args, "--rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let top_n: Option<usize> = parse_flag_value(args, "--top")
        .and_then(|s| s.parse().ok());
    let json_output = args.iter().any(|a| a == "--json");
    let weights_str = parse_flag_value(args, "--weights");

    let fitness_weights = match weights_str.as_deref() {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("--weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };

    // Collect .ag files from args (expand directories)
    let mut files = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            // Skip flags and their values
            continue;
        }
        // Skip values that follow flags
        if let Some(pos) = args.iter().position(|a| a == arg) {
            if pos > 0 {
                let prev = &args[pos - 1];
                if prev == "--rounds" || prev == "--top" || prev == "--weights" {
                    continue;
                }
            }
        }
        let path = std::path::Path::new(arg);
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                let mut dir_files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "ag"))
                    .map(|e| e.path().to_string_lossy().to_string())
                    .collect();
                dir_files.sort();
                files.extend(dir_files);
            }
        } else if path.extension().is_some_and(|ext| ext == "ag") || path.exists() {
            files.push(arg.clone());
        }
    }

    if files.is_empty() {
        return Err(AgentisError::General("no .ag files found".to_string()));
    }

    // Initialize evaluator dependencies
    let (store, refs) = ensure_initialized()?;
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let llm_backend = llm::create_backend(&cfg)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&root, &cfg);
    let tracer = trace::Tracer::new(trace::TraceLevel::Quiet);
    let audit_log = audit::AuditLog::open(&root);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let grant_pii = cfg.get("pii_transmit").is_some_and(|v| v == "allow");

    // Run each variant
    let mut all_entries: Vec<arena::ArenaEntry> = Vec::new();

    for file in &files {
        let mut round_entries = Vec::new();

        for _ in 0..rounds {
            let entry = run_arena_variant(
                file, &store, &refs, &root, &cfg, llm_backend.as_ref(),
                &io_ctx, &tracer, audit_log.as_ref(), max_agents,
                grant_pii, &fitness_weights,
            );
            round_entries.push(entry);
        }

        let entry = if rounds == 1 {
            round_entries.into_iter().next().unwrap()
        } else {
            arena::ArenaEntry::average(&round_entries)
        };
        all_entries.push(entry);
    }

    // Sort by score descending
    all_entries.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Apply --top filter
    if let Some(n) = top_n {
        all_entries.truncate(n);
    }

    // Output
    if json_output {
        println!("{}", arena::format_json(&all_entries, rounds));
    } else {
        print!("{}", arena::format_table(&all_entries, rounds));
    }

    Ok(())
}

/// Run a single variant and return its arena entry.
fn run_arena_variant(
    file: &str,
    store: &ObjectStore,
    refs: &Refs,
    root: &std::path::Path,
    _cfg: &config::Config,
    llm_backend: &dyn llm::LlmBackend,
    io_ctx: &io::IoContext,
    tracer: &trace::Tracer,
    audit_log: Option<&audit::AuditLog>,
    max_agents: u32,
    grant_pii: bool,
    weights: &fitness::FitnessWeights,
) -> arena::ArenaEntry {
    // Read source
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => return arena::ArenaEntry::from_error(file, &format!("{e}")),
    };

    // Parse
    let program = match Parser::parse_source(&source) {
        Ok(p) => p,
        Err(e) => return arena::ArenaEntry::from_error(file, &format!("{e}")),
    };

    // Commit (so VCS-dependent features work)
    let _ = store.save(&program).ok();

    // Create evaluator
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(store, refs)
        .with_persistence(store)
        .with_snapshot_registry(root)
        .with_llm(llm_backend)
        .with_io(io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(tracer);
    if let Some(audit) = audit_log {
        evaluator = evaluator.with_audit(audit);
    }
    evaluator.grant_all();
    if grant_pii {
        evaluator.grant(capabilities::CapKind::PiiTransmit);
    }

    // Run
    match evaluator.eval_program(&program) {
        Ok(_) => arena::ArenaEntry::from_report(file, &evaluator.fitness_report(), weights),
        Err(e) => {
            let mut report = evaluator.fitness_report();
            report.error = true;
            let mut entry = arena::ArenaEntry::from_report(file, &report, weights);
            entry.error = Some(arena::ArenaEntry::from_error(file, &format!("{e}")).error.unwrap());
            entry
        }
    }
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
