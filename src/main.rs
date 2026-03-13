mod arena;
mod ast;
mod audit;
mod bundle;
mod capabilities;
#[allow(dead_code)] // M37/M38 will use remaining methods
mod checkpoint;
mod colony;
mod compiler;
mod config;
mod error;
mod evaluator;
mod evolve;
mod fitness;
mod identity;
mod io;
mod json;
mod lexer;
#[allow(dead_code)] // exists() and rebuild_index() used in tests, CLI in M43
mod library;
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
                eprintln!(
                    "Usage: agentis go <source_file> [--trace] [--grant-pii] [--fitness] [--weights W]"
                );
                process::exit(1);
            }
            let force_verbose = args.iter().any(|a| a == "--trace");
            let grant_pii = args.iter().any(|a| a == "--grant-pii");
            let show_fitness = args.iter().any(|a| a == "--fitness");
            let weights = parse_flag_value(&args, "--weights");
            cmd_go(
                &args[2],
                force_verbose,
                grant_pii,
                show_fitness,
                weights.as_deref(),
            )
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
                eprintln!(
                    "Usage: agentis arena <files|dir> [--rounds N] [--top N] [--json] [--weights W]"
                );
                eprintln!("       agentis arena <files|dir> --parallel [--threads N]");
                eprintln!(
                    "       agentis arena <files|dir> --workers host1:port,host2:port [--secret S]"
                );
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
                eprintln!("  --mutate-prompt <s> Custom mutation prompt (string or file path)");
                eprintln!("  --weights W         Fitness weights (cb,val,exp)");
                eprintln!("  --budget-cap N      Max total CB across all evaluations");
                eprintln!("  --stop-on-stall N   Stop if no improvement for N generations");
                eprintln!("  --show-lineage      Show best agent's lineage");
                eprintln!("  --clean             Remove old fitness data before running");
                eprintln!("  --resume <ref>      Resume from checkpoint (hash prefix or tag)");
                eprintln!(
                    "  --checkpoint-interval N  Checkpoint every N generations (0=disable, default: 1)"
                );
                eprintln!("  --tag <name>        Tag the final checkpoint");
                eprintln!("  --dry-run           Estimate cost without running");
                eprintln!("  --threads N         Parallel arena evaluation (default: auto-detect)");
                eprintln!("  --workers W         Colony workers (addr:port,... or file path)");
                eprintln!("  --secret S          Colony auth secret");
                eprintln!("  --seed-from-lib <q>  Seed from library entries matching query");
                eprintln!("  --seed-top-k N       Take top N library entries (default: all)");
                eprintln!(
                    "  --warm-start-prob P  Library injection probability per slot (default: 0.3)"
                );
                eprintln!("  --warm-start-decay P Decay warm-start probability to P by final gen");
                eprintln!("  --adaptive-budget    Enable per-lineage adaptive budget allocation");
                eprintln!(
                    "  --max-lineage-fraction F  Max fraction for a single lineage (default: 0.5)"
                );
                eprintln!(
                    "  --lineage-stall-window N  Generations to assess improvement (default: 5)"
                );
                eprintln!(
                    "  --no-lib-add            Disable auto-add best to library at end of run"
                );
                eprintln!("  --lib-add-interval N    Also add to library every N generations");
                eprintln!("  --backup-to <dir>       Auto-export .agb bundle on each new best");
                eprintln!("  --resume-from <f.agb>   Import bundle and resume evolution from it");
                eprintln!();
                eprintln!("Event hooks (in .agentis/config):");
                eprintln!("  hooks.on_stagnation = reduce_budget 0.3");
                eprintln!("  hooks.on_new_best = checkpoint tag=improved");
                eprintln!("  hooks.on_crash = log variant crashed");
                eprintln!("  hooks.on_new_best = backup /backups/agent");
                eprintln!("  Actions: checkpoint, tag=<n>, lib_add, log <msg>,");
                eprintln!("           reduce_budget <f>, inject_library <n>, backup <dir>, skip");
                process::exit(1);
            }
            cmd_evolve(&args[2], &args[3..])
        }
        "worker" => cmd_worker(&args[2..]),
        "colony" => cmd_colony(&args[2..]),
        "memo" => cmd_memo(&args[2..]),
        "lib" => cmd_lib(&args[2..]),
        "identity" => cmd_identity(&args[2..]),
        "export" => cmd_export(&args[2..]),
        "import" => cmd_import(&args[2..]),
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
            println!("agentis v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "update" => cmd_update(&args[2..]),
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

    // Quiet background update check (at most once per day)
    if !matches!(args[1].as_str(), "update" | "version") {
        maybe_notify_update();
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
    eprintln!("  worker [addr:port]   Start colony worker (--secret S --max-concurrent N)");
    eprintln!("  colony status|ping   Colony diagnostics (--workers W --json)");
    eprintln!("  log [branch]         Show commit log for a branch");
    eprintln!("  snapshot list|show   List or inspect snapshots");
    eprintln!(
        "  arena <files|dir>    Rank variants by fitness (--rounds --top --json --parallel --workers)"
    );
    eprintln!("  mutate <file> [flags] Generate mutated agent variants");
    eprintln!(
        "  evolve <file> [flags] Evolutionary loop (-g N -n N --out dir --threads N --workers W)"
    );
    eprintln!("  lineage <file>       Trace variant ancestry back to seed");
    eprintln!(
        "  lib <subcommand>     Population library (add, list, show, search, remove, tags, tag)"
    );
    eprintln!("  audit [flags]        Show prompt audit log");
    eprintln!("  identity <sub>       Identity hash (hash, show, verify, diff)");
    eprintln!("  export --out <f.agb> Export agent bundle (.agb)");
    eprintln!("  import <f.agb>       Import agent bundle");
    eprintln!("  update [--check]     Self-update to the latest release");
    eprintln!("  version              Show version");
}

fn agentis_root() -> std::path::PathBuf {
    Path::new(".agentis").to_path_buf()
}

/// Parse a flag like `--weights 0.3,0.5,0.2` from args, returning the value after the flag.
fn parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// If the value is a path to an existing file, read its contents; otherwise return as-is.
fn resolve_template(value: &str) -> Result<String, AgentisError> {
    let path = Path::new(value);
    if path.is_file() {
        std::fs::read_to_string(path).map_err(|e| e.into())
    } else {
        Ok(value.to_string())
    }
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
        println!("Created examples/ directory with 13 programs.");
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
    ("functions.ag", include_str!("../examples/functions.ag")),
    ("collections.ag", include_str!("../examples/collections.ag")),
    ("budget.ag", include_str!("../examples/budget.ag")),
    ("classify.ag", include_str!("../examples/classify.ag")),
    ("io-sandbox.ag", include_str!("../examples/io-sandbox.ag")),
    ("pipeline.ag", include_str!("../examples/pipeline.ag")),
    ("parallel.ag", include_str!("../examples/parallel.ag")),
    ("explore.ag", include_str!("../examples/explore.ag")),
    ("test-suite.ag", include_str!("../examples/test-suite.ag")),
    ("pii-guard.ag", include_str!("../examples/pii-guard.ag")),
    ("evolve-seed.ag", include_str!("../examples/evolve-seed.ag")),
    (
        "self-improving-classifier.ag",
        include_str!("../examples/self-improving-classifier.ag"),
    ),
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

fn cmd_go(
    source_file: &str,
    force_verbose: bool,
    grant_pii: bool,
    show_fitness: bool,
    weights_str: Option<&str>,
) -> Result<(), AgentisError> {
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

    // Parse fitness weights: CLI flag > config > default
    let effective_weights_str = weights_str.map(|s| s.to_string());
    let cfg = config::Config::load(&agentis_root());
    let effective_weights_str =
        effective_weights_str.or_else(|| cfg.get("fitness.weights").map(|s| s.to_string()));
    let fitness_weights = match effective_weights_str.as_deref() {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };

    // Run
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&agentis_root(), &cfg);
    let trace_level = if force_verbose {
        trace::TraceLevel::Verbose
    } else {
        trace::TraceLevel::from_str(&cfg.get_or("trace.level", "normal"))
    };
    let tracer = trace::Tracer::new(trace_level);

    let audit_log = audit::AuditLog::open(&agentis_root());

    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let memo_dir = agentis_root().join("memo");
    let memo_max = cfg
        .get("memo.max_size")
        .and_then(parse_size_bytes)
        .unwrap_or(10 * 1024 * 1024);
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_snapshot_registry(&agentis_root())
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer)
        .with_memo_dir(&memo_dir)
        .with_memo_max_size(memo_max);
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

    let tree_hash = refs
        .resolve_tree(branch, &store)?
        .ok_or_else(|| AgentisError::General(format!("branch '{branch}' has no commits")))?;

    let program: ast::Program = store.load(&tree_hash)?;

    // Static type check (warnings only — does not block execution)
    let type_errors = typechecker::check(&program);
    for err in &type_errors {
        eprintln!("warning: {err}");
    }

    // Load config, LLM backend, I/O context, and tracer
    let cfg = config::Config::load(&agentis_root());
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;
    let io_ctx = io::IoContext::new(&agentis_root(), &cfg);
    let trace_level = trace::TraceLevel::from_str(&cfg.get_or("trace.level", "normal"));
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

    let tree_hash = refs
        .resolve_tree(branch, &store)?
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
    let result =
        network::sync_push_pull(&store, addr).map_err(|e| AgentisError::General(format!("{e}")))?;
    println!(
        "Sync complete: sent {}, received {}",
        result.sent, result.received
    );
    Ok(())
}

fn cmd_serve(addr: &str) -> Result<(), AgentisError> {
    let (store, _) = ensure_initialized()?;
    println!("Listening on {addr}...");
    let result = network::sync_serve_once(&store, addr)
        .map_err(|e| AgentisError::General(format!("{e}")))?;
    println!(
        "Sync complete: sent {}, received {}",
        result.sent, result.received
    );
    Ok(())
}

fn cmd_worker(args: &[String]) -> Result<(), AgentisError> {
    let addr = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .unwrap_or("0.0.0.0:9462");

    let secret = parse_flag_value(args, "--secret");
    let max_concurrent: usize = parse_flag_value(args, "--max-concurrent")
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(colony::detect_threads);
    let max_connections: usize = parse_flag_value(args, "--max-connections")
        .and_then(|s| s.parse().ok())
        .unwrap_or(64);

    // Also check config for colony.secret
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let secret = secret.or_else(|| cfg.get("colony.secret").map(|s| s.to_string()));

    let config = colony::WorkerConfig {
        addr: addr.to_string(),
        secret,
        max_concurrent,
        max_connections,
    };

    colony::run_worker(config).map_err(|e| AgentisError::General(format!("{e}")))
}

fn cmd_memo(args: &[String]) -> Result<(), AgentisError> {
    let root = agentis_root();
    let memo_dir = root.join("memo");
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("list");
    match subcmd {
        "list" => {
            let keys = evaluator::memo_list_keys(&memo_dir);
            if keys.is_empty() {
                println!("No memo keys found.");
                return Ok(());
            }
            println!("{:<30} {:>8} {:>10}", "KEY", "ENTRIES", "SIZE");
            println!("{}", "-".repeat(50));
            for (key, count, size) in &keys {
                println!("{:<30} {:>8} {:>10}", key, count, format_bytes(*size));
            }
            println!(
                "\n{} keys, {} total",
                keys.len(),
                format_bytes(evaluator::memo_store_total_size(&memo_dir))
            );
        }
        "stats" => {
            let keys = evaluator::memo_list_keys(&memo_dir);
            let total_size = evaluator::memo_store_total_size(&memo_dir);
            let total_entries: usize = keys.iter().map(|(_, c, _)| c).sum();
            println!("Memo store: {}", memo_dir.display());
            println!("  Keys:         {}", keys.len());
            println!("  Entries:      {total_entries}");
            println!("  Total size:   {}", format_bytes(total_size));
            if !keys.is_empty() {
                // Show top 5 largest keys
                let mut by_size = keys.clone();
                by_size.sort_by(|a, b| b.2.cmp(&a.2));
                println!("\n  Largest keys:");
                for (key, count, size) in by_size.iter().take(5) {
                    println!("    {:<30} {} entries, {}", key, count, format_bytes(*size));
                }
            }
        }
        "clear" => {
            if args.len() > 1 {
                // Clear specific key
                let key = &args[1];
                let memo_file = memo_dir.join(format!("{key}.jsonl"));
                if memo_file.exists() {
                    std::fs::remove_file(&memo_file)?;
                    println!("Cleared memo key: {key}");
                } else {
                    eprintln!("Memo key not found: {key}");
                }
            } else {
                // Clear all
                if memo_dir.exists() {
                    let keys = evaluator::memo_list_keys(&memo_dir);
                    let count = keys.len();
                    for (key, _, _) in &keys {
                        let path = memo_dir.join(format!("{key}.jsonl"));
                        let _ = std::fs::remove_file(path);
                    }
                    println!("Cleared {count} memo keys.");
                } else {
                    println!("No memo store found.");
                }
            }
        }
        _ => {
            eprintln!("Usage: agentis memo <list|stats|clear [key]>");
            process::exit(1);
        }
    }
    Ok(())
}

/// Parse a human-readable size string like "10MB", "1GB", "512KB" into bytes.
fn parse_size_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    if let Some(rest) = s.strip_suffix("GB") {
        rest.trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * 1024.0 * 1024.0 * 1024.0) as u64)
    } else if let Some(rest) = s.strip_suffix("MB") {
        rest.trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * 1024.0 * 1024.0) as u64)
    } else if let Some(rest) = s.strip_suffix("KB") {
        rest.trim().parse::<f64>().ok().map(|v| (v * 1024.0) as u64)
    } else if let Some(rest) = s.strip_suffix('B') {
        rest.trim().parse::<u64>().ok()
    } else {
        s.parse::<u64>().ok()
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn cmd_colony(args: &[String]) -> Result<(), AgentisError> {
    if args.is_empty() {
        eprintln!("Usage: agentis colony <subcommand>");
        eprintln!("  status [--workers W] [--secret S] [--json]  Show worker health");
        eprintln!("  ping <addr:port> [--secret S]               Ping a single worker");
        eprintln!("  history [--limit N]                          Show checkpoint chain");
        eprintln!("  trace <hash-or-tag>                         Show checkpoint details");
        eprintln!("  best [--min-score N]                        Find best checkpoint");
        eprintln!("  tags                                        List checkpoint tags");
        eprintln!("  tag <hash> <name>                           Tag a checkpoint");
        eprintln!("  gc [--older-than D] [--except-tagged] [--dry-run]  Garbage collect");
        process::exit(1);
    }

    let root = agentis_root();
    let cfg = config::Config::load(&root);

    match args[0].as_str() {
        "status" => {
            let workers_flag = parse_flag_value(args, "--workers");
            let secret_flag = parse_flag_value(args, "--secret");
            let json_output = args.iter().any(|a| a == "--json");

            let workers_str =
                workers_flag.or_else(|| cfg.get("colony.workers").map(|s| s.to_string()));
            let workers = match workers_str {
                Some(s) => colony::parse_workers(&s),
                None => {
                    eprintln!(
                        "No workers specified. Use --workers or set colony.workers in config."
                    );
                    process::exit(1);
                }
            };
            let secret = secret_flag.or_else(|| cfg.get("colony.secret").map(|s| s.to_string()));
            let connect_timeout = cfg.get_u64("colony.connect_timeout", 5) * 1000;

            let statuses = colony::colony_status(&workers, secret.as_deref(), connect_timeout);

            if json_output {
                println!("{}", colony::format_status_json(&statuses));
            } else {
                print!("{}", colony::format_status_table(&statuses));
            }

            Ok(())
        }
        "ping" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis colony ping <addr:port> [--secret S]");
                process::exit(1);
            }
            let addr = &args[1];
            let secret_flag = parse_flag_value(args, "--secret");
            let secret = secret_flag.or_else(|| cfg.get("colony.secret").map(|s| s.to_string()));
            let connect_timeout = cfg.get_u64("colony.connect_timeout", 5) * 1000;

            let status = colony::ping_worker(addr, secret.as_deref(), connect_timeout);
            print!("{}", colony::format_ping(&status));

            if status.status != "online" {
                process::exit(1);
            }
            Ok(())
        }
        "history" => {
            let store = checkpoint::CheckpointStore::new(&root);
            let limit: Option<usize> =
                parse_flag_value(args, "--limit").and_then(|s| s.parse().ok());

            let head = store
                .head()
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            match head {
                Some(h) => {
                    let chain = store
                        .walk_chain(&h, limit)
                        .map_err(|e| AgentisError::General(format!("{e}")))?;
                    // Attach tags to checkpoints for display
                    let tags = store
                        .list_tags()
                        .map_err(|e| AgentisError::General(format!("{e}")))?;
                    let tagged: Vec<(String, checkpoint::GenerationCheckpoint)> = chain
                        .into_iter()
                        .map(|(hash, mut ckpt)| {
                            if ckpt.tag.is_none() {
                                for (name, th) in &tags {
                                    if *th == hash {
                                        ckpt.tag = Some(name.clone());
                                        break;
                                    }
                                }
                            }
                            (hash, ckpt)
                        })
                        .collect();
                    print!("{}", checkpoint::format_history(&tagged));
                }
                None => {
                    println!("No checkpoints found. Run 'agentis evolve' first.");
                }
            }
            Ok(())
        }
        "trace" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis colony trace <hash-or-tag>");
                process::exit(1);
            }
            let store = checkpoint::CheckpointStore::new(&root);
            let hash = store
                .resolve(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            let mut ckpt = store
                .load(&hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            // Attach tag if not stored in checkpoint
            if ckpt.tag.is_none()
                && let Ok(tags) = store.list_tags()
            {
                for (name, th) in &tags {
                    if *th == hash {
                        ckpt.tag = Some(name.clone());
                        break;
                    }
                }
            }
            let parent_gen = ckpt
                .parent
                .as_ref()
                .and_then(|ph| store.load(ph).ok())
                .map(|pc| pc.generation);
            print!("{}", checkpoint::format_trace(&hash, &ckpt, parent_gen));
            Ok(())
        }
        "best" => {
            let store = checkpoint::CheckpointStore::new(&root);
            let min_score: Option<f64> =
                parse_flag_value(args, "--min-score").and_then(|s| s.parse().ok());

            let head = store
                .head()
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            match head {
                Some(h) => {
                    let chain = store
                        .walk_chain(&h, None)
                        .map_err(|e| AgentisError::General(format!("{e}")))?;
                    match checkpoint::find_best(&chain, min_score) {
                        Some((hash, ckpt)) => {
                            print!("{}", checkpoint::format_best(hash, ckpt));
                        }
                        None => {
                            if let Some(min) = min_score {
                                println!("No checkpoint with score >= {:.3} found.", min);
                            } else {
                                println!("No checkpoints found.");
                            }
                        }
                    }
                }
                None => {
                    println!("No checkpoints found. Run 'agentis evolve' first.");
                }
            }
            Ok(())
        }
        "tags" => {
            let store = checkpoint::CheckpointStore::new(&root);
            let tags = store
                .list_tags()
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            if tags.is_empty() {
                println!("No checkpoint tags.");
            } else {
                for (name, hash) in &tags {
                    println!("  {name:<24} {}", &hash[..12]);
                }
            }
            Ok(())
        }
        "tag" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis colony tag <hash> <name>");
                process::exit(1);
            }
            let store = checkpoint::CheckpointStore::new(&root);
            let hash = store
                .resolve(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            let name = &args[2];
            store
                .set_tag(name, &hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            println!("Tagged {} as '{name}'", &hash[..12]);
            Ok(())
        }
        "gc" => {
            let store = checkpoint::CheckpointStore::new(&root);
            let older_than_str = parse_flag_value(args, "--older-than");
            let except_tagged = args.iter().any(|a| a == "--except-tagged");
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let force = args.iter().any(|a| a == "--force");

            let older_than_ms = match older_than_str {
                Some(s) => match checkpoint::parse_duration_ms(&s) {
                    Some(ms) => Some(ms),
                    None => {
                        eprintln!("Invalid duration: '{s}'. Use e.g. '7d', '30d', '24h'.");
                        process::exit(1);
                    }
                },
                None => None,
            };

            let opts = checkpoint::GcOptions {
                older_than_ms,
                except_tagged,
                force,
                dry_run,
            };

            let result = store
                .gc(&opts)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            print!("{}", checkpoint::format_gc(&result, dry_run));
            Ok(())
        }
        other => {
            eprintln!("Unknown colony subcommand: {other}");
            eprintln!("  Available: status, ping, history, trace, best, tags, tag, gc");
            process::exit(1);
        }
    }
}

fn cmd_lib(args: &[String]) -> Result<(), AgentisError> {
    if args.is_empty() {
        eprintln!("Usage: agentis lib <subcommand>");
        eprintln!(
            "  add <file.ag> [--tag T] [--description D] [--no-desc]  Add variant to library"
        );
        eprintln!("  list [--tag T]                                          List library entries");
        eprintln!("  show <hash-or-tag>                                      Show entry details");
        eprintln!(
            "  search <query>                                          Search by description/tag"
        );
        eprintln!("  remove <hash-or-tag>                                    Remove entry");
        eprintln!("  tags                                                    List all tags");
        eprintln!("  tag <hash> <name>                                       Tag an entry");
        eprintln!("  export --out <file.alib> [--tag T] [--top N] [--all]    Export bundle");
        eprintln!("  import <file.alib> [--skip-duplicates]                  Import bundle");
        process::exit(1);
    }

    let root = agentis_root();

    match args[0].as_str() {
        "add" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!(
                    "Usage: agentis lib add <file.ag> [--tag T] [--description D] [--no-desc]"
                );
                process::exit(1);
            }
            let source_file = &args[1];
            let source = std::fs::read_to_string(source_file)?;
            let source_hash = evolve::hash_source(&source);

            let lib = library::LibraryStore::new(&root);

            // Check for duplicate source
            if lib.has_source(&source_hash).unwrap_or(false) {
                eprintln!("Source already in library (hash: {}...)", &source_hash[..8]);
                return Ok(());
            }

            let tag_name = parse_flag_value(args, "--tag");
            let no_desc = args.iter().any(|a| a == "--no-desc");
            let desc_flag = parse_flag_value(args, "--description");
            let desc_file = parse_flag_value(args, "--desc-from-file");

            // Resolve description
            let description = if no_desc {
                String::new()
            } else if let Some(d) = desc_flag {
                d
            } else if let Some(path) = desc_file {
                std::fs::read_to_string(&path)
                    .map_err(|e| AgentisError::General(format!("reading description file: {e}")))?
                    .trim()
                    .to_string()
            } else {
                // Try LLM-generated description via complete()
                let cfg = config::Config::load(&root);
                match llm::create_backend(&cfg) {
                    Ok(backend) => {
                        let result = backend.complete(
                            "Summarize what this Agentis program does in 1-2 sentences.",
                            &source,
                            &ast::TypeAnnotation::Named("string".to_string()),
                            None,
                        );
                        match result {
                            Ok(evaluator::Value::String(s)) => s.trim().to_string(),
                            Ok(_) => "(no description)".to_string(),
                            Err(_) => "(no description)".to_string(),
                        }
                    }
                    Err(_) => "(no description)".to_string(),
                }
            };

            // Evaluate source for fitness metrics
            let cfg = config::Config::load(&root);
            let weights = fitness::FitnessWeights::default();

            // Write source to temp file and evaluate via arena
            let tmp_dir =
                std::path::PathBuf::from(format!("/tmp/agentis-lib-add-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&tmp_dir);
            let tmp_file = tmp_dir.join("_lib_add.ag");
            std::fs::write(&tmp_file, &source)?;
            let grant_pii = cfg.get("pii_transmit").is_some_and(|v| v == "allow");
            let report = run_arena_variant_standalone(
                &tmp_file.to_string_lossy(),
                &root,
                grant_pii,
                &weights,
            );
            let _ = std::fs::remove_dir_all(&tmp_dir);

            // Parse tags from entry tags (inline in source)
            let mut entry_tags: Vec<String> = Vec::new();
            if let Some(ref t) = tag_name {
                entry_tags.push(t.clone());
            }

            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let lib_entry = library::LibraryEntry {
                source: source.clone(),
                source_hash: source_hash.clone(),
                seed_hash: source_hash.clone(),
                generation: 0,
                evolution_run: None,
                fitness_score: report.score,
                cb_efficiency: report.cb_eff,
                validate_rate: report.val_rate,
                explore_rate: report.exp_rate,
                prompt_count: report.prompt_count as u32,
                description,
                tags: entry_tags,
                timestamp: ts,
            };

            let hash = lib
                .store(&lib_entry)
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            if let Some(ref t) = tag_name {
                lib.set_tag(t, &hash)
                    .map_err(|e| AgentisError::General(format!("{e}")))?;
            }

            eprintln!(
                "Added to library: {}... (score: {:.3})",
                &hash[..12],
                report.score
            );
            Ok(())
        }
        "list" => {
            let lib = library::LibraryStore::new(&root);
            let tag_filter = parse_flag_value(args, "--tag");
            let hashes = lib
                .list()
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            let mut entries: Vec<(String, library::LibraryEntry)> = Vec::new();
            for hash in hashes {
                if let Ok(entry) = lib.load(&hash) {
                    if let Some(ref tag) = tag_filter
                        && !entry.tags.iter().any(|t| t == tag)
                    {
                        continue;
                    }
                    entries.push((hash, entry));
                }
            }
            // Sort by score descending
            entries.sort_by(|a, b| {
                b.1.fitness_score
                    .partial_cmp(&a.1.fitness_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            print!("{}", library::format_list(&entries));
            Ok(())
        }
        "show" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis lib show <hash-or-tag>");
                process::exit(1);
            }
            let lib = library::LibraryStore::new(&root);
            let hash = lib
                .resolve(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            let entry = lib
                .load(&hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            print!("{}", library::format_show(&hash, &entry));
            Ok(())
        }
        "search" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis lib search <query>");
                process::exit(1);
            }
            let lib = library::LibraryStore::new(&root);
            let results = lib
                .search(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            if results.is_empty() {
                println!("No matching entries found.");
            } else {
                print!("{}", library::format_list(&results));
            }
            Ok(())
        }
        "remove" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis lib remove <hash-or-tag>");
                process::exit(1);
            }
            let lib = library::LibraryStore::new(&root);
            let hash = lib
                .resolve(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            let removed = lib
                .remove(&hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            if removed {
                eprintln!("Removed: {}...", &hash[..12]);
            } else {
                eprintln!("Entry not found.");
            }
            Ok(())
        }
        "tags" => {
            let lib = library::LibraryStore::new(&root);
            let tags = lib
                .list_tags()
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            if tags.is_empty() {
                println!("No tags.");
            } else {
                for (name, hash) in &tags {
                    println!("{name}  →  {}...", &hash[..12.min(hash.len())]);
                }
            }
            Ok(())
        }
        "tag" => {
            if args.len() < 3 {
                eprintln!("Usage: agentis lib tag <hash> <name>");
                process::exit(1);
            }
            let lib = library::LibraryStore::new(&root);
            let hash = lib
                .resolve(&args[1])
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            lib.set_tag(&args[2], &hash)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            eprintln!("Tagged {}... as '{}'", &hash[..12], args[2]);
            Ok(())
        }
        "export" => {
            let out_path = parse_flag_value(args, "--out");
            let tag_filter = parse_flag_value(args, "--tag");
            let top_n: Option<usize> = parse_flag_value(args, "--top").and_then(|s| s.parse().ok());
            let export_all = args.iter().any(|a| a == "--all");

            let out_file = match out_path {
                Some(p) => p,
                None => {
                    eprintln!(
                        "Usage: agentis lib export --out <file.alib> [--tag T] [--top N] [--all]"
                    );
                    process::exit(1);
                }
            };

            let lib = library::LibraryStore::new(&root);
            let all_hashes = lib
                .list()
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            // Select entries
            let mut selected: Vec<(String, library::LibraryEntry)> = Vec::new();
            for hash in &all_hashes {
                if let Ok(entry) = lib.load(hash) {
                    if let Some(ref tag) = tag_filter
                        && !entry.tags.iter().any(|t| t == tag)
                    {
                        continue;
                    }
                    selected.push((hash.clone(), entry));
                }
            }

            // Sort by fitness score descending for --top
            selected.sort_by(|a, b| {
                b.1.fitness_score
                    .partial_cmp(&a.1.fitness_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(n) = top_n {
                selected.truncate(n);
            }

            if selected.is_empty() && !export_all {
                eprintln!("No matching entries to export.");
                process::exit(1);
            }

            let hashes: Vec<String> = if export_all && selected.is_empty() {
                // --all with no filter: export everything
                all_hashes
            } else {
                selected.iter().map(|(h, _)| h.clone()).collect()
            };

            if hashes.is_empty() {
                eprintln!("No entries to export.");
                process::exit(1);
            }

            let count = lib
                .export_bundle(&hashes, std::path::Path::new(&out_file))
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            eprintln!("Exported {} entries to {}", count, out_file);
            Ok(())
        }
        "import" => {
            if args.len() < 2 || args[1].starts_with('-') {
                eprintln!("Usage: agentis lib import <file.alib> [--skip-duplicates]");
                process::exit(1);
            }
            let bundle_path = &args[1];
            let skip_dups = args.iter().any(|a| a == "--skip-duplicates");

            let lib = library::LibraryStore::new(&root);
            lib.init()
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            let (imported, skipped) = lib
                .import_bundle(std::path::Path::new(bundle_path), skip_dups)
                .map_err(|e| AgentisError::General(format!("{e}")))?;

            eprintln!("Imported: {}", imported);
            if skipped > 0 {
                eprintln!("Skipped (duplicates): {}", skipped);
            }
            Ok(())
        }
        other => {
            eprintln!("Unknown lib subcommand: {other}");
            eprintln!("  Available: add, list, show, search, remove, tags, tag, export, import");
            process::exit(1);
        }
    }
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
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "ag"))
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
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;
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
                if fail_fast {
                    break;
                }
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
                if fail_fast {
                    break;
                }
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
            let dot_str: String = std::iter::repeat_n('.', dots).collect();
            println!("  {kind_label} {dot_str} {status}");

            if !outcome.passed
                && let Some(ref detail) = outcome.detail
                && verbose
            {
                println!("    {detail}");
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
    println!(
        "Results: {} passed, {} failed, {} total",
        total_passed, total_failed, total
    );

    if any_file_failed {
        process::exit(1);
    }
    Ok(())
}

fn cmd_repl(args: &[String]) -> Result<(), AgentisError> {
    let (store, refs) = ensure_initialized()?;
    let root = agentis_root();

    let cfg = config::Config::load(&root);
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;
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
    let branch_name = refs
        .current_branch()
        .unwrap_or_else(|_| "genesis".to_string());
    if let Ok(Some(tree_hash)) = refs.resolve_tree(&branch_name, &store)
        && let Ok(program) = store.load::<ast::Program>(&tree_hash)
    {
        for decl in &program.declarations {
            match decl {
                ast::Declaration::Function(f) => {
                    let _ = evaluator.eval_repl_declaration(&ast::Declaration::Function(f.clone()));
                }
                ast::Declaration::Agent(a) => {
                    let _ = evaluator.eval_repl_declaration(&ast::Declaration::Agent(a.clone()));
                }
                ast::Declaration::Type(t) => {
                    let _ = evaluator.eval_repl_declaration(&ast::Declaration::Type(t.clone()));
                }
                _ => {}
            }
        }
    }

    // --resume <hash>: restore snapshot with CB penalty
    let resume_hash = args.windows(2).find_map(|w| {
        if w[0] == "--resume" {
            Some(w[1].clone())
        } else {
            None
        }
    });
    if let Some(ref prefix) = resume_hash {
        let full_hash = resolve_snapshot_hash(&root, prefix)?;
        let mgr = snapshot::SnapshotManager::new(&store).with_registry(&root);
        let snap = mgr
            .load(&full_hash)
            .map_err(|e| AgentisError::General(format!("{e}")))?;
        evaluator.restore_snapshot_with_penalty(&snap);
        eprintln!("Restored snapshot {}", &full_hash[..12]);
        eprintln!(
            "  Budget: {} (after 30% resurrection tax)",
            evaluator.budget_remaining()
        );
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
                            if let ast::Declaration::Statement(ast::Statement::Let(l)) = &decl
                                && let Some(v) = evaluator.lookup_var(&l.name)
                            {
                                println!("{v}");
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
            println!("{:<14} {:<12} {:<9} SCOPES", "HASH", "CB", "OUTPUT");
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
        _ => {
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
fn resolve_snapshot_hash(
    agentis_root: &std::path::Path,
    prefix: &str,
) -> Result<String, AgentisError> {
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
        let agents = mutation::extract_agents(&source).map_err(AgentisError::General)?;
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
    let custom_template = parse_flag_value(args, "--mutate-prompt")
        .map(|s| resolve_template(&s))
        .transpose()?;
    let dry_run = args.iter().any(|a| a == "--dry-run");

    // Load LLM backend from config
    let root = agentis_root();
    let cfg = config::Config::load(&root);
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;

    if dry_run {
        // Dry-run: show what would be generated without writing files
        let agents = mutation::extract_agents(&source).map_err(AgentisError::General)?;
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
                        agents
                            .iter()
                            .map(|a| a.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
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
                mutation::llm_mutate(
                    &agent.instruction,
                    llm_backend.as_ref(),
                    custom_template.as_deref(),
                )
                .map_err(AgentisError::General)?
            };
            println!(
                "{}",
                mutation::format_dry_run(
                    i,
                    count,
                    &agent.name,
                    &agent.instruction,
                    &new_instruction
                )
            );
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
    )
    .map_err(AgentisError::General)?;

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
    let out_dir = parse_flag_value(args, "--out").unwrap_or_else(|| "evolved".to_string());
    let agent_filter = parse_flag_value(args, "--agent");
    let custom_template = parse_flag_value(args, "--mutate-prompt")
        .map(|s| resolve_template(&s))
        .transpose()?;
    let weights_str = parse_flag_value(args, "--weights");
    let budget_cap: Option<u64> =
        parse_flag_value(args, "--budget-cap").and_then(|s| s.parse().ok());
    let stop_on_stall: Option<usize> =
        parse_flag_value(args, "--stop-on-stall").and_then(|s| s.parse().ok());
    let show_lineage = args.iter().any(|a| a == "--show-lineage");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let clean = args.iter().any(|a| a == "--clean");
    let threads: Option<usize> = parse_flag_value(args, "--threads").and_then(|s| s.parse().ok());
    let workers_flag = parse_flag_value(args, "--workers");
    let secret_flag = parse_flag_value(args, "--secret");
    let resume_ref = parse_flag_value(args, "--resume");
    let checkpoint_interval: usize = parse_flag_value(args, "--checkpoint-interval")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let tag_name = parse_flag_value(args, "--tag");
    let seed_from_lib = parse_flag_value(args, "--seed-from-lib");
    let seed_top_k: Option<usize> =
        parse_flag_value(args, "--seed-top-k").and_then(|s| s.parse().ok());
    let warm_start_prob_flag: Option<f64> =
        parse_flag_value(args, "--warm-start-prob").and_then(|s| s.parse().ok());
    let warm_start_decay: Option<f64> =
        parse_flag_value(args, "--warm-start-decay").and_then(|s| s.parse().ok());
    let adaptive_budget = args.iter().any(|a| a == "--adaptive-budget");
    let max_lineage_fraction: f64 = parse_flag_value(args, "--max-lineage-fraction")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.5);
    let lineage_stall_window: usize = parse_flag_value(args, "--lineage-stall-window")
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let no_lib_add = args.iter().any(|a| a == "--no-lib-add");
    let lib_add_interval: Option<usize> =
        parse_flag_value(args, "--lib-add-interval").and_then(|s| s.parse().ok());
    let memo_max_size_flag: Option<String> = parse_flag_value(args, "--memo-max-size");
    let backup_to = parse_flag_value(args, "--backup-to");
    let resume_from_bundle = parse_flag_value(args, "--resume-from");

    // Error if both --resume and --resume-from specified
    if resume_ref.is_some() && resume_from_bundle.is_some() {
        return Err(AgentisError::General(
            "Cannot specify both --resume and --resume-from".to_string(),
        ));
    }

    // If --resume-from, import the bundle first, then use its checkpoint hash
    let resume_ref = if let Some(ref bundle_path) = resume_from_bundle {
        let root = agentis_root();
        let contents = bundle::read_bundle(bundle_path)?;
        let result = bundle::import_to_store(&contents, &root, bundle::MemoConflict::Append)?;
        let hash = result.checkpoint_hash.ok_or_else(|| {
            AgentisError::General("Bundle has no checkpoint to resume from".to_string())
        })?;
        eprintln!(
            "Imported bundle: {} (checkpoint: {}...)",
            bundle_path,
            &hash[..12]
        );
        Some(hash)
    } else {
        resume_ref
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
    let mut cfg = config::Config::load(&root);
    // CLI --memo-max-size overrides config
    if let Some(ref size_str) = memo_max_size_flag {
        cfg.set("memo.max_size", size_str);
    }

    // Parse fitness weights: CLI flag > config > default
    let weights_str = weights_str.or_else(|| cfg.get("fitness.weights").map(|s| s.to_string()));
    let fitness_weights = match weights_str.as_deref() {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };
    let llm_backend =
        llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;

    // Auto-add threshold from config (default 0.8)
    let min_auto_score: f64 = cfg
        .get("lib.min_auto_score")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.8);

    // Parse event hooks (fail early on invalid syntax)
    let hooks = evolve::parse_hooks(&cfg).map_err(AgentisError::General)?;
    if !hooks.is_empty() {
        eprintln!(
            "Event hooks: {}",
            hooks
                .iter()
                .map(|h| format!("{:?}", h.event).to_lowercase())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // Dry-run mode
    if dry_run {
        let agents = mutation::extract_agents(&seed_source).map_err(AgentisError::General)?;
        let prompt_count = agents.len().max(1); // rough estimate: at least 1 prompt per agent
        let avg_instruction_len = if agents.is_empty() {
            30
        } else {
            agents.iter().map(|a| a.instruction.len()).sum::<usize>() / agents.len()
        };
        let tps = cfg
            .get("ollama_tokens_per_second")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(30.0);
        print!(
            "{}",
            evolve::format_dry_run(
                generations,
                population,
                prompt_count,
                llm_backend.name(),
                avg_instruction_len,
                tps,
            )
        );
        return Ok(());
    }

    // Initialize evaluator dependencies
    let (store, refs) = ensure_initialized()?;
    let io_ctx = io::IoContext::new(&root, &cfg);
    let tracer = trace::Tracer::new(trace::TraceLevel::Quiet);
    let audit_log = audit::AuditLog::open(&root);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let grant_pii = cfg.get("pii_transmit").is_some_and(|v| v == "allow");

    // Resolve colony workers
    let workers_str = workers_flag.or_else(|| cfg.get("colony.workers").map(|s| s.to_string()));
    let colony_workers: Vec<String> = workers_str
        .map(|s| colony::parse_workers(&s))
        .unwrap_or_default();
    let colony_secret = secret_flag.or_else(|| cfg.get("colony.secret").map(|s| s.to_string()));
    let use_colony = !colony_workers.is_empty();

    let colony_cfg = if use_colony {
        Some(colony::ColonyConfig {
            workers: colony_workers.clone(),
            secret: colony_secret,
            connect_timeout_ms: cfg.get_u64("colony.connect_timeout", 5) * 1000,
            eval_timeout_ms: cfg.get_u64("colony.eval_timeout", 120) * 1000,
        })
    } else {
        None
    };

    // Create output directory
    let out_path = std::path::PathBuf::from(&out_dir);
    std::fs::create_dir_all(&out_path)?;

    // Fitness storage directory
    let fitness_dir = root.join("fitness");

    // Clean old per-generation JSONL files if --clean
    if clean && fitness_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&fitness_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "jsonl") {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        eprintln!("Cleaned old fitness data from {}", fitness_dir.display());
    }

    // Checkpoint store
    let ckpt_store = checkpoint::CheckpointStore::new(&root);

    // Load library seeds if --seed-from-lib specified
    let library_seeds: Vec<(String, String)> = if let Some(ref query) = seed_from_lib {
        let lib_store = library::LibraryStore::new(&root);
        let search_query = query.strip_prefix("tag:").unwrap_or(query);
        let results = lib_store.search(search_query).unwrap_or_default();
        let limited: Vec<_> = if let Some(k) = seed_top_k {
            results.into_iter().take(k).collect()
        } else {
            results
        };
        if limited.is_empty() {
            eprintln!("Warning: no library entries found for query '{query}'");
        }
        limited
            .iter()
            .map(|(_, entry)| (entry.source.clone(), entry.source_hash.clone()))
            .collect()
    } else {
        vec![]
    };
    let initial_warm_prob =
        warm_start_prob_flag.unwrap_or(if !library_seeds.is_empty() { 0.3 } else { 0.0 });

    // Resume from checkpoint or start fresh
    let (
        start_gen,
        mut parents,
        mut best_ever_score,
        mut best_ever_source,
        mut best_ever_hash,
        mut stall_count,
        mut cumulative_cb,
        mut first_gen_avg_prompts,
        mut prev_ckpt_hash,
        mut ancestor_failures,
        mut ancestor_successes,
    ) = if let Some(ref resume) = resume_ref {
        let hash = ckpt_store
            .resolve(resume)
            .map_err(|e| AgentisError::General(format!("resume: {e}")))?;
        let ckpt = ckpt_store
            .load(&hash)
            .map_err(|e| AgentisError::General(format!("resume: {e}")))?;

        // Warn if seed hash differs
        if ckpt.seed_hash != seed_hash {
            eprintln!(
                "Warning: seed hash differs from checkpoint (checkpoint: {}..., current: {}...)",
                &ckpt.seed_hash[..8.min(ckpt.seed_hash.len())],
                &seed_hash[..8.min(seed_hash.len())]
            );
        }

        let parents_vec: Vec<(String, String)> = ckpt
            .parents
            .iter()
            .map(|p| (p.source.clone(), p.source_hash.clone()))
            .collect();

        // Restore ancestor history from checkpoint (M45)
        let restored_failures: Vec<evolve::AncestorRecord> = ckpt
            .ancestor_failures
            .iter()
            .map(|r| evolve::AncestorRecord {
                generation: r.generation as usize,
                outcome: r.outcome.clone(),
                fitness_score: r.fitness_score,
                code_hash: r.code_hash.clone(),
                elapsed_ms: r.elapsed_ms,
            })
            .collect();
        let restored_successes: Vec<evolve::AncestorRecord> = ckpt
            .ancestor_successes
            .iter()
            .map(|r| evolve::AncestorRecord {
                generation: r.generation as usize,
                outcome: r.outcome.clone(),
                fitness_score: r.fitness_score,
                code_hash: r.code_hash.clone(),
                elapsed_ms: r.elapsed_ms,
            })
            .collect();

        eprintln!(
            "Resuming from checkpoint {} (gen {})",
            &hash[..12],
            ckpt.generation
        );
        (
            ckpt.generation as usize + 1,
            parents_vec,
            ckpt.best_ever_score,
            ckpt.best_ever_source.clone(),
            ckpt.best_ever_hash.clone(),
            ckpt.stall_count as usize,
            ckpt.cumulative_cb,
            ckpt.first_gen_avg_prompts,
            Some(hash),
            restored_failures,
            restored_successes,
        )
    } else {
        (
            1,
            vec![(seed_source.clone(), seed_hash.clone())],
            0.0,
            seed_source.clone(),
            seed_hash.clone(),
            0,
            0u64,
            0.0,
            None,
            Vec::new(),
            Vec::new(),
        )
    };

    let end_gen = start_gen + generations - 1;

    // Best-ever fitness components (for auto-lib-add)
    let mut best_ever_cb_eff = 0.0f64;
    let mut best_ever_val_rate = 0.0f64;
    let mut best_ever_exp_rate = 0.0f64;
    let mut best_ever_prompts = 0u32;
    let mut best_ever_gen = 0u32;

    // Create PRNG for warm-start randomization
    let rng_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut rng = evolve::SimpleRng::new(rng_seed);

    // Adaptive budget manager
    let mut budget_mgr = if adaptive_budget {
        let mut mgr = evolve::AdaptiveBudgetManager::new(evolve::AdaptiveBudgetConfig {
            window_size: lineage_stall_window,
            max_fraction: max_lineage_fraction,
            min_improvement: 0.01,
        });
        mgr.register_lineage(&seed_hash);
        for (_, lh) in &library_seeds {
            mgr.register_lineage(lh);
        }
        Some(mgr)
    } else {
        None
    };

    // Lineage tracking: source_hash → lineage_seed_hash
    let mut variant_lineages: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    variant_lineages.insert(seed_hash.clone(), seed_hash.clone());
    for (_, lh) in &library_seeds {
        variant_lineages.insert(lh.clone(), lh.clone());
    }

    eprintln!("Evolution: {}", source_file);
    if resume_ref.is_some() {
        eprintln!(
            "  Population: {}, Generations: {} (gen {}..{})",
            population, generations, start_gen, end_gen
        );
    } else {
        eprintln!("  Population: {}, Generations: {}", population, generations);
    }
    if !library_seeds.is_empty() {
        eprintln!(
            "  Library seeds: {}{}",
            library_seeds.len(),
            seed_from_lib
                .as_ref()
                .map(|q| format!(" (query: \"{q}\")"))
                .unwrap_or_default()
        );
        if let Some(end) = warm_start_decay {
            eprintln!(
                "  Warm-start: {:.0}% → {:.0}%",
                initial_warm_prob * 100.0,
                end * 100.0
            );
        } else if initial_warm_prob > 0.0 {
            eprintln!("  Warm-start: {:.0}%", initial_warm_prob * 100.0);
        }
    }
    if adaptive_budget {
        eprintln!(
            "  Adaptive budget: window={}, max-fraction={:.0}%",
            lineage_stall_window,
            max_lineage_fraction * 100.0
        );
    }
    eprintln!();

    let mut gen_results: Vec<evolve::GenResult> = Vec::new();
    let mut cb_only_warned = false;
    let mut prev_terminated_count = 0usize;

    for g in start_gen..=end_gen {
        // Compute warm-start probability with decay
        let current_warm_prob = if let Some(end_prob) = warm_start_decay {
            if end_gen > start_gen {
                let progress = (g - start_gen) as f64 / (end_gen - start_gen) as f64;
                initial_warm_prob + (end_prob - initial_warm_prob) * progress
            } else {
                initial_warm_prob
            }
        } else {
            initial_warm_prob
        };
        let default_provenance = if g == 1 && resume_ref.is_none() {
            "seed-file"
        } else {
            "population"
        };

        // Expand parents by adaptive budget allocation (if enabled)
        let alloc_parents;
        let effective_parents = if let Some(ref mgr) = budget_mgr {
            let allocation = mgr.allocate_slots(population);
            alloc_parents = evolve::expand_parents_by_allocation(
                &parents,
                &allocation,
                &variant_lineages,
                &seed_hash,
            );
            &alloc_parents
        } else {
            &parents
        };

        // Generate variants from parents
        let mock_offset = (g - 1) * population;
        let tracked_variants = evolve::generate_tracked_variants(
            effective_parents,
            population,
            g,
            &base_name,
            agent_filter.as_deref(),
            llm_backend.as_ref(),
            custom_template.as_deref(),
            mock_offset,
            default_provenance,
            &library_seeds,
            current_warm_prob,
            &mut rng,
        )
        .map_err(AgentisError::General)?;

        // Write variant files to temp for arena evaluation
        let gen_dir = out_path.join(format!("g{g:02}"));
        std::fs::create_dir_all(&gen_dir)?;

        for v in &tracked_variants {
            let path = gen_dir.join(&v.filename);
            std::fs::write(&path, &v.source)?;
        }

        // Arena: evaluate each variant (colony, parallel, or sequential)
        let variant_files: Vec<String> = tracked_variants
            .iter()
            .map(|v| gen_dir.join(&v.filename).to_string_lossy().to_string())
            .collect();

        let variant_entries = if let Some(ref cc) = colony_cfg {
            colony::run_arena_colony(
                &variant_files,
                1,
                cc,
                &root,
                grant_pii,
                &fitness_weights,
                DEFAULT_BUDGET,
            )
        } else if let Some(tc) = threads {
            run_arena_parallel(&variant_files, 1, &root, grant_pii, &fitness_weights, tc)
        } else {
            let mut entries = Vec::new();
            for v in &tracked_variants {
                let path = gen_dir.join(&v.filename);
                let entry = run_arena_variant(
                    &path.to_string_lossy(),
                    &store,
                    &refs,
                    &root,
                    &cfg,
                    llm_backend.as_ref(),
                    &io_ctx,
                    &tracer,
                    audit_log.as_ref(),
                    max_agents,
                    grant_pii,
                    &fitness_weights,
                );
                entries.push(entry);
            }
            entries
        };

        let mut scored: Vec<(evolve::TrackedVariant, arena::ArenaEntry)> = Vec::new();
        let mut lineage_cb_spent: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut lineage_best_scores: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();

        for (v, entry) in tracked_variants.iter().zip(variant_entries.into_iter()) {
            // Track variant lineage
            let lineage = if v.provenance == "library" {
                v.parent_hash.clone()
            } else {
                variant_lineages
                    .get(&v.parent_hash)
                    .cloned()
                    .unwrap_or_else(|| seed_hash.clone())
            };
            variant_lineages.insert(v.source_hash.clone(), lineage.clone());

            // Track CB usage (global + per-lineage)
            if entry.error.is_none() {
                let cb_spent = ((1.0 - entry.cb_eff) * DEFAULT_BUDGET as f64) as u64;
                cumulative_cb += cb_spent;
                *lineage_cb_spent.entry(lineage.clone()).or_insert(0) += cb_spent;
            }

            // Track per-lineage best score
            let best = lineage_best_scores.entry(lineage).or_insert(0.0);
            if entry.score > *best {
                *best = entry.score;
            }

            // Fire on_crash / on_validation_fail hooks
            if entry.error.is_some() {
                for hook in evolve::hooks_for_event(&hooks, &evolve::HookEvent::Crash) {
                    for action in &hook.actions {
                        match action {
                            evolve::HookAction::Log(msg) => {
                                eprintln!("  [hook] crash: {}", msg);
                            }
                            evolve::HookAction::Skip => {
                                // skip is a no-op here — variant already scored 0
                            }
                            _ => {} // other actions handled at generation level
                        }
                    }
                }
            } else if entry.val_rate < 1.0 {
                for hook in evolve::hooks_for_event(&hooks, &evolve::HookEvent::ValidationFail) {
                    for action in &hook.actions {
                        match action {
                            evolve::HookAction::Log(msg) => {
                                eprintln!("  [hook] validation_fail: {}", msg);
                            }
                            evolve::HookAction::Skip => {}
                            _ => {}
                        }
                    }
                }
            }

            scored.push((v.clone(), entry));
        }

        // Update adaptive budget manager with per-lineage results
        if let Some(ref mut mgr) = budget_mgr {
            for (lineage_hash, best) in &lineage_best_scores {
                let cb = lineage_cb_spent.get(lineage_hash).copied().unwrap_or(0);
                mgr.update(lineage_hash, *best, cb);
            }
        }

        // Sort by score descending
        scored.sort_by(|a, b| {
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Compute generation stats
        let successful: Vec<&arena::ArenaEntry> = scored
            .iter()
            .map(|(_, e)| e)
            .filter(|e| e.error.is_none())
            .collect();
        let gen_best = scored.first().map(|(_, e)| e.score).unwrap_or(0.0);
        let gen_avg = if successful.is_empty() {
            0.0
        } else {
            successful.iter().map(|e| e.score).sum::<f64>() / successful.len() as f64
        };
        let gen_avg_prompts = if successful.is_empty() {
            0.0
        } else {
            successful
                .iter()
                .map(|e| e.prompt_count as f64)
                .sum::<f64>()
                / successful.len() as f64
        };

        if g == start_gen && first_gen_avg_prompts == 0.0 {
            first_gen_avg_prompts = gen_avg_prompts;
        }

        // CB-only warning (once)
        if !cb_only_warned {
            let all_cb_only = scored
                .iter()
                .all(|(_, e)| e.val_rate >= 1.0 && e.exp_rate >= 1.0 && e.error.is_none());
            if all_cb_only && !successful.is_empty() {
                eprintln!("  Warning: No validate/explore blocks — fitness = CB efficiency only.");
                cb_only_warned = true;
            }
        }

        // Record ancestor history (M45) — classify each variant as success or failure
        for (v, entry) in &scored {
            let outcome = if entry.error.is_some() {
                let err_msg = entry.error.as_deref().unwrap_or("unknown");
                if err_msg.contains("CognitiveOverload") || err_msg.contains("budget") {
                    "cb_exhausted"
                } else if err_msg.contains("timeout") || err_msg.contains("Timeout") {
                    "timeout"
                } else {
                    "validation_failed"
                }
            } else if entry.val_rate < 1.0 {
                "validation_failed"
            } else {
                "survived"
            };
            let record = evolve::AncestorRecord {
                generation: g,
                outcome: outcome.to_string(),
                fitness_score: entry.score,
                code_hash: v.source_hash.clone(),
                elapsed_ms: entry.eval_time_ms.unwrap_or(0),
            };
            if outcome == "survived" {
                ancestor_successes.insert(0, record);
                if ancestor_successes.len() > 3 {
                    ancestor_successes.truncate(3);
                }
            } else {
                ancestor_failures.insert(0, record);
                if ancestor_failures.len() > 10 {
                    ancestor_failures.truncate(10);
                }
            }
        }

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
        let is_new_best = gen_best > best_ever_score;
        if is_new_best {
            best_ever_score = gen_best;
            best_ever_source = best_variant.source.clone();
            best_ever_hash = best_variant.source_hash.clone();
            best_ever_cb_eff = scored[0].1.cb_eff;
            best_ever_val_rate = scored[0].1.val_rate;
            best_ever_exp_rate = scored[0].1.exp_rate;
            best_ever_prompts = scored[0].1.prompt_count as u32;
            best_ever_gen = g as u32;
            stall_count = 0;
        } else {
            stall_count += 1;
        }

        // Fire on_new_best hooks
        if is_new_best {
            for hook in evolve::hooks_for_event(&hooks, &evolve::HookEvent::NewBest) {
                for action in &hook.actions {
                    match action {
                        evolve::HookAction::Checkpoint => {
                            // Force a checkpoint (handled below via hook_force_checkpoint)
                        }
                        evolve::HookAction::Tag(_) => {
                            // Tag will be applied to checkpoint created this generation
                            // (deferred to checkpoint creation below)
                        }
                        evolve::HookAction::LibAdd => {
                            let lib_store = library::LibraryStore::new(&root);
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            let lib_entry = library::LibraryEntry {
                                source: best_variant.source.clone(),
                                source_hash: best_variant.source_hash.clone(),
                                seed_hash: seed_hash.clone(),
                                generation: g as u32,
                                evolution_run: prev_ckpt_hash.clone(),
                                fitness_score: gen_best,
                                cb_efficiency: scored[0].1.cb_eff,
                                validate_rate: scored[0].1.val_rate,
                                explore_rate: scored[0].1.exp_rate,
                                prompt_count: scored[0].1.prompt_count as u32,
                                description: format!("auto-added by on_new_best hook (gen {})", g),
                                tags: vec!["hook-best".to_string()],
                                timestamp: ts,
                            };
                            if let Err(e) = lib_store.store(&lib_entry) {
                                eprintln!("  [hook] lib_add failed: {}", e);
                            } else {
                                eprintln!(
                                    "  [hook] added best to library (score: {:.3})",
                                    gen_best
                                );
                            }
                        }
                        evolve::HookAction::Backup(dir) => {
                            let id_hash = identity::identity_from_seed(&seed_hash);
                            match bundle::write_evolve_backup(
                                dir,
                                g as u32,
                                &seed_hash,
                                &best_variant.source,
                                None,
                                &root,
                                &id_hash,
                                &[],
                            ) {
                                Ok(p) => {
                                    let size = std::fs::metadata(&p)
                                        .map(|m| m.len())
                                        .unwrap_or(0);
                                    eprintln!(
                                        "  backup → {} ({} KB)",
                                        p.display(),
                                        size / 1024
                                    );
                                }
                                Err(e) => eprintln!("  [hook] backup failed: {}", e),
                            }
                        }
                        evolve::HookAction::Log(msg) => {
                            eprintln!("  [hook] new_best: {}", msg);
                        }
                        _ => {}
                    }
                }
            }
        }

        // --backup-to flag: write backup on new best
        if is_new_best
            && let Some(ref backup_dir) = backup_to
        {
            let id_hash = identity::identity_from_seed(&seed_hash);
            match bundle::write_evolve_backup(
                backup_dir,
                g as u32,
                &seed_hash,
                &best_variant.source,
                None,
                &root,
                &id_hash,
                &[],
            ) {
                Ok(p) => {
                    let size = std::fs::metadata(&p)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    eprintln!(
                        "  backup → {} ({} KB)",
                        p.display(),
                        size / 1024
                    );
                }
                Err(e) => eprintln!("  Warning: backup failed: {}", e),
            }
        }

        // Fire on_stagnation hooks
        if stall_count > 0 {
            for hook in evolve::hooks_for_event(&hooks, &evolve::HookEvent::Stagnation) {
                for action in &hook.actions {
                    match action {
                        evolve::HookAction::ReduceBudget(frac) => {
                            if let Some(ref mut mgr) = budget_mgr {
                                // Reduce all lineage fractions by the given factor
                                mgr.reduce_all(*frac);
                                eprintln!(
                                    "  [hook] reduced budget fractions by {:.0}%",
                                    frac * 100.0
                                );
                            }
                        }
                        evolve::HookAction::InjectLibrary(count) => {
                            let lib_store = library::LibraryStore::new(&root);
                            let results = lib_store.search("").unwrap_or_default();
                            let inject_count = (*count).min(results.len());
                            if inject_count > 0 {
                                // Replace lowest-scoring parents with library entries
                                let inject_start = parents.len().saturating_sub(inject_count);
                                for (i, (_, entry)) in results.iter().take(inject_count).enumerate()
                                {
                                    let idx = inject_start + i;
                                    if idx < parents.len() {
                                        parents[idx] =
                                            (entry.source.clone(), entry.source_hash.clone());
                                        variant_lineages.insert(
                                            entry.source_hash.clone(),
                                            entry.source_hash.clone(),
                                        );
                                    }
                                }
                                eprintln!(
                                    "  [hook] injected {} library entries into parent pool",
                                    inject_count
                                );
                            }
                        }
                        evolve::HookAction::Log(msg) => {
                            eprintln!("  [hook] stagnation (stall={}): {}", stall_count, msg);
                        }
                        evolve::HookAction::Checkpoint => {}
                        evolve::HookAction::Tag(_) => {}
                        _ => {}
                    }
                }
            }
        }

        // Determine if hooks force a checkpoint or tag
        let hook_force_checkpoint = if is_new_best {
            evolve::hooks_for_event(&hooks, &evolve::HookEvent::NewBest)
                .iter()
                .any(|h| {
                    h.actions
                        .iter()
                        .any(|a| matches!(a, evolve::HookAction::Checkpoint))
                })
        } else {
            false
        };
        let hook_tag: Option<String> = if is_new_best {
            evolve::hooks_for_event(&hooks, &evolve::HookEvent::NewBest)
                .iter()
                .flat_map(|h| h.actions.iter())
                .find_map(|a| {
                    if let evolve::HookAction::Tag(name) = a {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
        } else {
            None
        };

        // Convergence warning
        if stall_count >= 3 && (stop_on_stall.is_none() || stall_count < stop_on_stall.unwrap()) {
            eprintln!(
                "  Warning: Evolution stalled at generation {} (score: {:.3})",
                g, best_ever_score
            );
        }

        // Detect early stop conditions (checked after checkpoint below)
        let stop_stall = stop_on_stall.is_some_and(|limit| stall_count >= limit);
        let stop_budget = budget_cap.is_some_and(|cap| cumulative_cb >= cap);

        // Select top K = N/2 as parents for next generation
        let k = (population / 2).max(1);
        parents = scored
            .iter()
            .take(k)
            .map(|(v, _)| (v.source.clone(), v.source_hash.clone()))
            .collect();

        // Auto-checkpoint (always checkpoint on last gen, early stop, or hook-forced)
        let is_last = g == end_gen || stop_stall || stop_budget;
        let should_checkpoint = hook_force_checkpoint
            || (checkpoint_interval > 0 && (g % checkpoint_interval == 0 || is_last));
        let ckpt_hash_str = if should_checkpoint {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let gen_ckpt = checkpoint::GenerationCheckpoint {
                generation: g as u32,
                parent: prev_ckpt_hash.clone(),
                seed_hash: seed_hash.clone(),
                parents: parents
                    .iter()
                    .map(|(s, h)| checkpoint::ParentEntry {
                        source: s.clone(),
                        source_hash: h.clone(),
                    })
                    .collect(),
                best_ever_score,
                best_ever_source: best_ever_source.clone(),
                best_ever_hash: best_ever_hash.clone(),
                stall_count: stall_count as u32,
                cumulative_cb,
                first_gen_avg_prompts,
                gen_best_score: gen_best,
                gen_avg_score: gen_avg,
                gen_avg_prompts,
                variant_count: tracked_variants.len() as u32,
                timestamp: ts,
                tag: None,
                ancestor_failures: ancestor_failures
                    .iter()
                    .map(|r| checkpoint::CheckpointAncestorRecord {
                        generation: r.generation as u32,
                        outcome: r.outcome.clone(),
                        fitness_score: r.fitness_score,
                        code_hash: r.code_hash.clone(),
                        elapsed_ms: r.elapsed_ms,
                    })
                    .collect(),
                ancestor_successes: ancestor_successes
                    .iter()
                    .map(|r| checkpoint::CheckpointAncestorRecord {
                        generation: r.generation as u32,
                        outcome: r.outcome.clone(),
                        fitness_score: r.fitness_score,
                        code_hash: r.code_hash.clone(),
                        elapsed_ms: r.elapsed_ms,
                    })
                    .collect(),
            };
            let hash = ckpt_store
                .store(&gen_ckpt)
                .map_err(|e| AgentisError::General(format!("checkpoint: {e}")))?;
            ckpt_store
                .set_head(&hash)
                .map_err(|e| AgentisError::General(format!("checkpoint HEAD: {e}")))?;
            prev_ckpt_hash = Some(hash.clone());
            // Apply hook-generated tag
            if let Some(ref ht) = hook_tag {
                if let Err(e) = ckpt_store.set_tag(ht, &hash) {
                    eprintln!("  [hook] tag failed: {}", e);
                } else {
                    eprintln!("  [hook] tagged checkpoint as '{}'", ht);
                }
            }
            Some(hash)
        } else {
            None
        };

        // Print generation summary with provenance breakdown
        let (prov_seed, prov_pop, prov_lib) = evolve::count_provenance(&tracked_variants);
        let prov_suffix = if prov_lib > 0 {
            let mut parts = Vec::new();
            if prov_seed > 0 {
                parts.push(format!("{} seed-file", prov_seed));
            }
            if prov_pop > 0 {
                parts.push(format!("{} population", prov_pop));
            }
            parts.push(format!("{} library", prov_lib));
            format!("{} variants: {}", tracked_variants.len(), parts.join(", "))
        } else {
            format!("{} variants", tracked_variants.len())
        };
        if let Some(ref h) = ckpt_hash_str {
            eprintln!(
                "Gen {:>2}: best={:.3}  avg={:.3}  prompts={:.1}  ckpt={}  ({})",
                g,
                gen_best,
                gen_avg,
                gen_avg_prompts,
                &h[..8],
                prov_suffix
            );
        } else {
            eprintln!(
                "Gen {:>2}: best={:.3}  avg={:.3}  prompts={:.1}  ({})",
                g, gen_best, gen_avg, gen_avg_prompts, prov_suffix
            );
        }

        // Show adaptive budget report
        if let Some(ref mgr) = budget_mgr {
            if mgr.active_count() > 1 || mgr.terminated_lineages().len() > prev_terminated_count {
                eprintln!("        budget: {}", mgr.format_report(population));
            }
            // Report newly terminated lineages
            let terminated = mgr.terminated_lineages();
            for t in terminated.iter().skip(prev_terminated_count) {
                let last = mgr
                    .last_score(&t.seed_hash)
                    .map(|s| format!("{:.3}", s))
                    .unwrap_or_else(|| "N/A".to_string());
                eprintln!(
                    "  Lineage {}.. terminated (stalled {} gens at {})",
                    &t.seed_hash[..8.min(t.seed_hash.len())],
                    t.stall_count,
                    last
                );
            }
            prev_terminated_count = terminated.len();
        }

        // Periodic auto-add to library (--lib-add-interval N)
        if !no_lib_add
            && is_new_best
            && best_ever_score >= min_auto_score
            && lib_add_interval.is_some_and(|interval| g % interval == 0)
        {
            let lib_store = library::LibraryStore::new(&root);
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let lib_entry = library::LibraryEntry {
                source: best_ever_source.clone(),
                source_hash: best_ever_hash.clone(),
                seed_hash: seed_hash.clone(),
                generation: best_ever_gen,
                evolution_run: prev_ckpt_hash.clone(),
                fitness_score: best_ever_score,
                cb_efficiency: best_ever_cb_eff,
                validate_rate: best_ever_val_rate,
                explore_rate: best_ever_exp_rate,
                prompt_count: best_ever_prompts,
                description: format!("auto-added at gen {} (interval)", g),
                tags: vec!["auto-evolve".to_string()],
                timestamp: ts,
            };
            if !lib_store.has_source(&best_ever_hash).unwrap_or(true) {
                match lib_store.store(&lib_entry) {
                    Ok(h) => eprintln!(
                        "  Auto-added to library: {}... (score: {:.3})",
                        &h[..12],
                        best_ever_score
                    ),
                    Err(e) => eprintln!("  Warning: auto-lib-add failed: {}", e),
                }
            }
        }

        // Early stop (after checkpoint so last gen is always saved)
        if stop_stall {
            eprintln!(
                "\nStopped: no improvement for {} generations (score: {:.3})",
                stop_on_stall.unwrap(),
                best_ever_score
            );
            break;
        }
        if stop_budget {
            eprintln!(
                "\nBudget cap reached at generation {} ({}/{} CB spent)",
                g,
                cumulative_cb,
                budget_cap.unwrap()
            );
            break;
        }
    }

    // Tag the final checkpoint if --tag specified
    if let Some(ref tag) = tag_name
        && let Some(ref hash) = prev_ckpt_hash
    {
        ckpt_store
            .set_tag(tag, hash)
            .map_err(|e| AgentisError::General(format!("tag: {e}")))?;
        eprintln!("Tagged checkpoint {} as '{tag}'", &hash[..12]);
    }

    // Write best-of-run
    let best_filename = format!("{}-best.ag", base_name);
    let best_path = out_path.join(&best_filename);
    std::fs::write(&best_path, &best_ever_source)?;

    eprintln!();
    eprintln!(
        "Best agent: {} (score: {:.3})",
        best_path.display(),
        best_ever_score
    );

    // Show lineage if requested
    if show_lineage {
        let lineage = evolve::load_lineage(&fitness_dir);
        let chain = evolve::trace_lineage(&lineage, &best_ever_hash, source_file);
        if chain.len() > 1 {
            eprintln!("  Lineage: {}", evolve::format_lineage(&chain));
        }
    }

    // Auto-add best-ever to library at end of run
    if !no_lib_add && best_ever_score >= min_auto_score {
        let lib_store = library::LibraryStore::new(&root);
        if !lib_store.has_source(&best_ever_hash).unwrap_or(true) {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let lib_entry = library::LibraryEntry {
                source: best_ever_source.clone(),
                source_hash: best_ever_hash.clone(),
                seed_hash: seed_hash.clone(),
                generation: best_ever_gen,
                evolution_run: prev_ckpt_hash.clone(),
                fitness_score: best_ever_score,
                cb_efficiency: best_ever_cb_eff,
                validate_rate: best_ever_val_rate,
                explore_rate: best_ever_exp_rate,
                prompt_count: best_ever_prompts,
                description: format!("auto-added from evolve (gen {})", best_ever_gen),
                tags: vec!["auto-evolve".to_string()],
                timestamp: ts,
            };
            match lib_store.store(&lib_entry) {
                Ok(h) => eprintln!(
                    "  Added to library: {}... (score: {:.3})",
                    &h[..12],
                    best_ever_score
                ),
                Err(e) => eprintln!("  Warning: auto-lib-add failed: {}", e),
            }
        }
    }

    // Efficiency summary
    if !gen_results.is_empty() && first_gen_avg_prompts > 0.0 {
        let last_avg_prompts = gen_results.last().unwrap().avg_prompts;
        if last_avg_prompts != first_gen_avg_prompts {
            let pct = ((last_avg_prompts - first_gen_avg_prompts) / first_gen_avg_prompts) * 100.0;
            if pct < 0.0 {
                eprintln!(
                    "  Efficiency: prompt calls {:.0}% ({:.1} → {:.1} avg)",
                    pct, first_gen_avg_prompts, last_avg_prompts
                );
            } else {
                eprintln!(
                    "  Efficiency: prompt calls +{:.0}% ({:.1} → {:.1} avg)",
                    pct, first_gen_avg_prompts, last_avg_prompts
                );
            }
        }
    }

    Ok(())
}

fn cmd_identity(args: &[String]) -> Result<(), AgentisError> {
    if args.is_empty() {
        eprintln!("Usage: agentis identity <subcommand>");
        eprintln!("  hash [file.ag]       Compute identity hash (from HEAD checkpoint or seed file)");
        eprintln!("  show                 Show identity card");
        eprintln!("  verify <file.agb>    Verify bundle integrity + identity");
        eprintln!("  diff <a.agb> <b.agb> Compare two bundles");
        return Ok(());
    }

    match args[0].as_str() {
        "hash" => {
            if let Some(file) = args.get(1) {
                // Seed-only identity from file
                let source = std::fs::read_to_string(file)?;
                let seed_hash = evolve::hash_source(&source);
                let id = identity::identity_from_seed(&seed_hash);
                println!("{}", id);
            } else {
                // From HEAD checkpoint
                let root = agentis_root();
                let ckpt_store = checkpoint::CheckpointStore::new(&root);
                let head = ckpt_store
                    .head()
                    .map_err(|e| AgentisError::General(format!("{e}")))?
                    .ok_or_else(|| {
                        AgentisError::General(
                            "No HEAD checkpoint. Run 'agentis evolve' first or provide a file."
                                .to_string(),
                        )
                    })?;
                let id = identity::identity_from_checkpoint(&head, &ckpt_store)
                    .map_err(AgentisError::General)?;
                println!("{}", id);
            }
        }
        "show" => {
            let root = agentis_root();
            let ckpt_store = checkpoint::CheckpointStore::new(&root);
            let head = ckpt_store
                .head()
                .map_err(|e| AgentisError::General(format!("{e}")))?
                .ok_or_else(|| {
                    AgentisError::General("No HEAD checkpoint found.".to_string())
                })?;
            let ckpt = ckpt_store
                .load(&head)
                .map_err(|e| AgentisError::General(format!("{e}")))?;
            let id = identity::identity_from_checkpoint(&head, &ckpt_store)
                .map_err(AgentisError::General)?;

            // Collect tags pointing to HEAD
            let tags = ckpt_store.list_tags().unwrap_or_default();
            let head_tags: Vec<&str> = tags
                .iter()
                .filter(|(_, h)| h == &head)
                .map(|(n, _)| n.as_str())
                .collect();

            eprintln!("Identity Card");
            eprintln!("  Hash:       {}", id);
            eprintln!("  Seed:       {}...", &ckpt.seed_hash[..12.min(ckpt.seed_hash.len())]);
            eprintln!("  Generation: {}", ckpt.generation);
            eprintln!("  Best score: {:.3}", ckpt.best_ever_score);
            if !head_tags.is_empty() {
                eprintln!("  Tags:       {}", head_tags.join(", "));
            }
            // Drift hint: if stall_count is high, warn
            if ckpt.stall_count >= 5 {
                eprintln!(
                    "  Drift risk: HIGH (stalled {} generations)",
                    ckpt.stall_count
                );
            } else if ckpt.stall_count >= 2 {
                eprintln!(
                    "  Drift risk: moderate (stalled {} generations)",
                    ckpt.stall_count
                );
            } else {
                eprintln!("  Drift risk: low");
            }
        }
        "verify" => {
            let path = args.get(1).ok_or_else(|| {
                AgentisError::General("Usage: agentis identity verify <file.agb>".to_string())
            })?;
            let report = bundle::verify_bundle(path)?;
            if report.root_hash_ok {
                eprintln!("PASS  root hash: {}", &report.computed_root[..16]);
            } else {
                eprintln!(
                    "FAIL  root hash: expected {}, got {}",
                    &report.stored_root[..16],
                    &report.computed_root[..16]
                );
            }
            if report.identity_ok {
                eprintln!("PASS  identity: {}", &report.identity_hash[..16]);
            } else {
                eprintln!(
                    "FAIL  identity: stored {}, recomputed {}",
                    &report.stored_identity[..16.min(report.stored_identity.len())],
                    &report.identity_hash[..16.min(report.identity_hash.len())]
                );
            }
            if !report.root_hash_ok || !report.identity_ok {
                std::process::exit(1);
            }
        }
        "diff" => {
            if args.len() < 3 {
                return Err(AgentisError::General(
                    "Usage: agentis identity diff <a.agb> <b.agb>".to_string(),
                ));
            }
            let a = bundle::read_bundle(&args[1])?;
            let b = bundle::read_bundle(&args[2])?;

            let same_seed = a.identity.seed_hash == b.identity.seed_hash;
            eprintln!("Bundle A: {}", &args[1]);
            eprintln!("  Identity: {}...", &a.identity.identity_hash[..16]);
            eprintln!("  Seed:     {}...", &a.identity.seed_hash[..12.min(a.identity.seed_hash.len())]);
            eprintln!("  Gen:      {}", a.identity.generation);
            eprintln!();
            eprintln!("Bundle B: {}", &args[2]);
            eprintln!("  Identity: {}...", &b.identity.identity_hash[..16]);
            eprintln!("  Seed:     {}...", &b.identity.seed_hash[..12.min(b.identity.seed_hash.len())]);
            eprintln!("  Gen:      {}", b.identity.generation);
            eprintln!();

            if same_seed {
                let gen_delta = (b.identity.generation as i64) - (a.identity.generation as i64);
                eprintln!("Same seed: yes");
                eprintln!("Generation delta: {gen_delta:+}");
            } else {
                eprintln!("Same seed: no");
            }
        }
        other => {
            return Err(AgentisError::General(format!(
                "Unknown identity subcommand: {other}"
            )));
        }
    }
    Ok(())
}

fn cmd_export(args: &[String]) -> Result<(), AgentisError> {
    let out_path = parse_flag_value(args, "--out").ok_or_else(|| {
        AgentisError::General("Usage: agentis export --out <file.agb> [--include-memos] [--tag T] [--lineage-depth N]".to_string())
    })?;
    let include_memos = args.iter().any(|a| a == "--include-memos");
    let tag_name = parse_flag_value(args, "--tag");
    let lineage_depth: Option<usize> =
        parse_flag_value(args, "--lineage-depth").and_then(|s| s.parse().ok());

    let root = agentis_root();
    let ckpt_store = checkpoint::CheckpointStore::new(&root);

    // Load HEAD checkpoint
    let head_hash = ckpt_store
        .head()
        .map_err(|e| AgentisError::General(format!("{e}")))?
        .ok_or_else(|| {
            AgentisError::General(
                "No HEAD checkpoint. Run 'agentis evolve' first.".to_string(),
            )
        })?;
    let ckpt = ckpt_store
        .load(&head_hash)
        .map_err(|e| AgentisError::General(format!("{e}")))?;

    // Compute identity
    let id_hash = identity::identity_from_checkpoint(&head_hash, &ckpt_store)
        .map_err(AgentisError::General)?;

    // Collect tags for this checkpoint
    let all_tags = ckpt_store.list_tags().unwrap_or_default();
    let ckpt_tags: Vec<String> = all_tags
        .iter()
        .filter(|(_, h)| h == &head_hash)
        .map(|(n, _)| n.clone())
        .collect();

    // Build identity section
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let bundle_identity = bundle::BundleIdentity {
        seed_hash: ckpt.seed_hash.clone(),
        generation: ckpt.generation,
        identity_hash: id_hash.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tags: ckpt_tags,
        timestamp: ts,
    };

    // Seed source from best_ever_source
    let seed_source = ckpt.best_ever_source.clone();

    // Checkpoint data
    let ckpt_data = Some(ckpt.to_bytes());

    // Memos
    let memos = if include_memos {
        let memo_dir = root.join("memo");
        bundle::collect_memos(&memo_dir)?
    } else {
        Vec::new()
    };

    // Lineage JSONL files
    let fitness_dir = root.join("fitness");
    let lineage = bundle::collect_lineage(&fitness_dir, lineage_depth)?;

    // Write bundle
    bundle::write_bundle(
        &out_path,
        &bundle_identity,
        &seed_source,
        ckpt_data.as_deref(),
        &memos,
        &lineage,
    )?;

    // Apply tag if requested
    if let Some(ref tag) = tag_name {
        let _ = ckpt_store.set_tag(tag, &head_hash);
    }

    let file_size = std::fs::metadata(&out_path)
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!("Exported: {}", out_path);
    eprintln!("  Identity:   {}...", &id_hash[..16]);
    eprintln!("  Generation: {}", ckpt.generation);
    eprintln!("  Size:       {} bytes", file_size);

    Ok(())
}

fn cmd_import(args: &[String]) -> Result<(), AgentisError> {
    if args.is_empty() || args[0].starts_with('-') {
        return Err(AgentisError::General(
            "Usage: agentis import <file.agb> [--as <name>] [--memo-conflict skip|append|replace]"
                .to_string(),
        ));
    }

    let bundle_path = &args[0];
    let tag_as = parse_flag_value(args, "--as");
    let memo_conflict = parse_flag_value(args, "--memo-conflict")
        .unwrap_or_else(|| "append".to_string());

    let conflict_mode = match memo_conflict.as_str() {
        "skip" => bundle::MemoConflict::Skip,
        "append" => bundle::MemoConflict::Append,
        "replace" => bundle::MemoConflict::Replace,
        other => {
            return Err(AgentisError::General(format!(
                "Unknown memo-conflict mode: {other}. Use skip, append, or replace."
            )));
        }
    };

    let root = agentis_root();

    // Read and validate bundle
    let contents = bundle::read_bundle(bundle_path)?;

    // Import to store
    let result = bundle::import_to_store(&contents, &root, conflict_mode)?;

    // Tag checkpoint if --as specified
    if let Some(ref tag) = tag_as
        && let Some(ref ckpt_hash) = result.checkpoint_hash
    {
        let ckpt_store = checkpoint::CheckpointStore::new(&root);
        ckpt_store
            .set_tag(tag, ckpt_hash)
            .map_err(|e| AgentisError::General(format!("tag: {e}")))?;
    }

    eprintln!("Imported: {}", bundle_path);
    eprintln!(
        "  Identity: {}...",
        &contents.identity.identity_hash[..16.min(contents.identity.identity_hash.len())]
    );
    if let Some(ref h) = result.checkpoint_hash {
        eprintln!("  Checkpoint: {}...", &h[..12]);
    }
    eprintln!("  Memos restored: {}", result.memo_keys_restored);
    eprintln!("  Lineage files:  {}", result.lineage_files_restored);

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
        eprintln!(
            "Source hash {} not found in lineage data.",
            &source_hash[..12]
        );
        return Ok(());
    }

    println!("{}", evolve::format_lineage(&chain));
    Ok(())
}

/// Collect .ag files from CLI args, expanding directories and skipping flags.
fn collect_ag_files(args: &[String]) -> Vec<String> {
    let flags_with_values = [
        "--rounds",
        "--top",
        "--weights",
        "--threads",
        "--workers",
        "--secret",
    ];
    let mut files = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if arg.starts_with('-') {
            continue;
        }
        // Skip values that follow flags with arguments
        if i > 0 && flags_with_values.contains(&args[i - 1].as_str()) {
            continue;
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
    files
}

fn cmd_arena(args: &[String]) -> Result<(), AgentisError> {
    // Parse flags
    let rounds: usize = parse_flag_value(args, "--rounds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let top_n: Option<usize> = parse_flag_value(args, "--top").and_then(|s| s.parse().ok());
    let json_output = args.iter().any(|a| a == "--json");
    let weights_str = parse_flag_value(args, "--weights");
    let parallel = args.iter().any(|a| a == "--parallel");
    let threads: Option<usize> = parse_flag_value(args, "--threads").and_then(|s| s.parse().ok());
    let workers_flag = parse_flag_value(args, "--workers");
    let secret_flag = parse_flag_value(args, "--secret");

    let files = collect_ag_files(args);
    if files.is_empty() {
        return Err(AgentisError::General("no .ag files found".to_string()));
    }

    // Initialize evaluator dependencies
    let (store, refs) = ensure_initialized()?;
    let root = agentis_root();
    let cfg = config::Config::load(&root);

    // Parse fitness weights: CLI flag > config > default
    let weights_str = weights_str.or_else(|| cfg.get("fitness.weights").map(|s| s.to_string()));
    let fitness_weights = match weights_str.as_deref() {
        Some(s) => fitness::FitnessWeights::parse(s)
            .map_err(|e| AgentisError::General(format!("weights: {e}")))?,
        None => fitness::FitnessWeights::default(),
    };

    let grant_pii = cfg.get("pii_transmit").is_some_and(|v| v == "allow");

    // Resolve workers: CLI flag > config
    let workers_str = workers_flag.or_else(|| cfg.get("colony.workers").map(|s| s.to_string()));
    let workers: Vec<String> = workers_str
        .map(|s| colony::parse_workers(&s))
        .unwrap_or_default();
    let secret = secret_flag.or_else(|| cfg.get("colony.secret").map(|s| s.to_string()));

    let use_colony = !workers.is_empty();
    let use_parallel = parallel || threads.is_some();

    let mut all_entries: Vec<arena::ArenaEntry> = if use_colony {
        // Colony mode: distribute across workers
        let colony_cfg = colony::ColonyConfig {
            workers: workers.clone(),
            secret,
            connect_timeout_ms: cfg.get_u64("colony.connect_timeout", 5) * 1000,
            eval_timeout_ms: cfg.get_u64("colony.eval_timeout", 120) * 1000,
        };
        colony::run_arena_colony(
            &files,
            rounds,
            &colony_cfg,
            &root,
            grant_pii,
            &fitness_weights,
            DEFAULT_BUDGET,
        )
    } else if use_parallel {
        let thread_count = threads.unwrap_or_else(colony::detect_threads);
        eprintln!(
            "Parallel arena: {} variants, {} threads, {} round{} each",
            files.len(),
            thread_count,
            rounds,
            if rounds == 1 { "" } else { "s" }
        );
        run_arena_parallel(
            &files,
            rounds,
            &root,
            grant_pii,
            &fitness_weights,
            thread_count,
        )
    } else {
        // Sequential (original behavior)
        let llm_backend =
            llm::create_backend(&cfg).map_err(|e| AgentisError::General(format!("{e}")))?;
        let io_ctx = io::IoContext::new(&root, &cfg);
        let tracer = trace::Tracer::new(trace::TraceLevel::Quiet);
        let audit_log = audit::AuditLog::open(&root);
        let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;

        let mut entries = Vec::new();
        for file in &files {
            let mut round_entries = Vec::new();
            for _ in 0..rounds {
                let entry = run_arena_variant(
                    file,
                    &store,
                    &refs,
                    &root,
                    &cfg,
                    llm_backend.as_ref(),
                    &io_ctx,
                    &tracer,
                    audit_log.as_ref(),
                    max_agents,
                    grant_pii,
                    &fitness_weights,
                );
                round_entries.push(entry);
            }
            let entry = if rounds == 1 {
                round_entries.into_iter().next().unwrap()
            } else {
                arena::ArenaEntry::average(&round_entries)
            };
            entries.push(entry);
        }
        entries
    };

    // Sort by score descending, then by filename for tie-breaking
    all_entries.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file.cmp(&b.file))
    });

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

    // Colony stats
    if use_colony {
        eprintln!(
            "{}",
            arena::format_colony_stats(&all_entries, workers.len())
        );
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_arena_variant(
    file: &str,
    store: &ObjectStore,
    refs: &Refs,
    root: &std::path::Path,
    cfg: &config::Config,
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
    let memo_dir = root.join("memo");
    let memo_max = cfg
        .get("memo.max_size")
        .and_then(parse_size_bytes)
        .unwrap_or(10 * 1024 * 1024);
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(store, refs)
        .with_persistence(store)
        .with_snapshot_registry(root)
        .with_llm(llm_backend)
        .with_io(io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(tracer)
        .with_memo_dir(&memo_dir)
        .with_memo_max_size(memo_max);
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
            entry.error = Some(
                arena::ArenaEntry::from_error(file, &format!("{e}"))
                    .error
                    .unwrap(),
            );
            entry
        }
    }
}

/// Run arena variants in parallel using a thread pool (M31).
///
/// Each thread creates its own evaluator context from the .agentis root.
/// Results are collected via channels and returned in the original file order.
fn run_arena_parallel(
    files: &[String],
    rounds: usize,
    root: &std::path::Path,
    grant_pii: bool,
    weights: &fitness::FitnessWeights,
    thread_count: usize,
) -> Vec<arena::ArenaEntry> {
    let pool = colony::ThreadPool::new(thread_count);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, arena::ArenaEntry)>();

    for (idx, file) in files.iter().enumerate() {
        for _ in 0..rounds {
            let tx = tx.clone();
            let file = file.clone();
            let weights = weights.clone();
            let root_path = root.to_path_buf();

            pool.execute(move || {
                let entry = run_arena_variant_standalone(&file, &root_path, grant_pii, &weights);
                let _ = tx.send((idx, entry));
            });
        }
    }
    drop(tx); // close sender so rx.iter() terminates after all jobs

    // Collect results grouped by file index
    let mut grouped: std::collections::HashMap<usize, Vec<arena::ArenaEntry>> =
        std::collections::HashMap::new();
    for (idx, entry) in rx {
        grouped.entry(idx).or_default().push(entry);
    }

    pool.join();

    // Build final entries in original file order, averaging rounds
    let mut results = Vec::with_capacity(files.len());
    for (idx, file) in files.iter().enumerate() {
        let entries = grouped.remove(&idx).unwrap_or_default();
        let entry = if entries.len() == 1 {
            entries.into_iter().next().unwrap()
        } else if entries.is_empty() {
            arena::ArenaEntry::from_error(file, "no results")
        } else {
            arena::ArenaEntry::average(&entries)
        };
        results.push(entry);
    }

    results
}

/// Run a single arena variant in a standalone context (for parallel use).
///
/// Creates its own Config, LLM backend, ObjectStore, etc. from the
/// agentis root path. This makes the function fully self-contained
/// and safe to call from any thread.
fn run_arena_variant_standalone(
    file: &str,
    root: &std::path::Path,
    grant_pii: bool,
    weights: &fitness::FitnessWeights,
) -> arena::ArenaEntry {
    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => return arena::ArenaEntry::from_error(file, &format!("{e}")),
    };

    let program = match Parser::parse_source(&source) {
        Ok(p) => p,
        Err(e) => return arena::ArenaEntry::from_error(file, &format!("{e}")),
    };

    let cfg = config::Config::load(root);
    let llm_backend = match llm::create_backend(&cfg) {
        Ok(b) => b,
        Err(e) => return arena::ArenaEntry::from_error(file, &format!("{e}")),
    };
    let io_ctx = io::IoContext::new(root, &cfg);
    let tracer = trace::Tracer::new(trace::TraceLevel::Quiet);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;

    let store = ObjectStore::new(root);
    let refs = Refs::new(root);
    let _ = store.save(&program).ok();

    let memo_dir = root.join("memo");
    let memo_max = cfg
        .get("memo.max_size")
        .and_then(parse_size_bytes)
        .unwrap_or(10 * 1024 * 1024);
    let mut evaluator = Evaluator::new(DEFAULT_BUDGET)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer)
        .with_memo_dir(&memo_dir)
        .with_memo_max_size(memo_max);
    evaluator.grant_all();
    if grant_pii {
        evaluator.grant(capabilities::CapKind::PiiTransmit);
    }

    match evaluator.eval_program(&program) {
        Ok(_) => arena::ArenaEntry::from_report(file, &evaluator.fitness_report(), weights),
        Err(e) => {
            let mut report = evaluator.fitness_report();
            report.error = true;
            let mut entry = arena::ArenaEntry::from_report(file, &report, weights);
            entry.error = Some(
                arena::ArenaEntry::from_error(file, &format!("{e}"))
                    .error
                    .unwrap(),
            );
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
            if let Some(ref agent) = agent_filter
                && entry.agent != *agent
            {
                continue;
            }

            entries.push(entry);
        }
    }

    // Take last N
    let start = if entries.len() > last_n {
        entries.len() - last_n
    } else {
        0
    };
    let entries = &entries[start..];

    if entries.is_empty() {
        println!("No matching audit entries.");
        return Ok(());
    }

    // Print table header
    println!(
        "{:<12} {:<16} {:<18} {:<10} BACKEND",
        "TIME", "AGENT", "PII", "STATUS"
    );

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

        println!(
            "{:<12} {:<16} {:<18} {:<10} {}",
            time_str, agent, pii_display, status, backend
        );
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
            agent: val
                .get("agent")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
            pii_scan: val
                .get("pii_scan")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
            pii_types: val
                .get("pii_types")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            pii_transmit_granted: val
                .get("pii_transmit_granted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            backend: val
                .get("backend")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            model: val
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        }
    }
}

// --- Self-Update ---

fn cmd_update(args: &[String]) -> Result<(), AgentisError> {
    let check_only = args.iter().any(|a| a == "--check");

    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: v{current}");

    // Query GitHub releases API
    print!("Checking for updates...");
    let mut response = ureq::get("https://api.github.com/repos/Replikanti/agentis/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "agentis-self-update")
        .call()
        .map_err(|e| AgentisError::General(format!("failed to check for updates: {e}")))?;

    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|e| AgentisError::General(format!("failed to read response: {e}")))?;

    let release = json::parse(&body)
        .map_err(|e| AgentisError::General(format!("failed to parse release info: {e}")))?;

    let tag = release
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AgentisError::General("missing tag_name in release".into()))?;

    let latest = tag.strip_prefix('v').unwrap_or(tag);

    match compare_versions(current, latest) {
        std::cmp::Ordering::Equal => {
            println!(" already up to date (v{current}).");
            return Ok(());
        }
        std::cmp::Ordering::Greater => {
            println!(" local version is newer than release (v{current} > v{latest}).");
            return Ok(());
        }
        std::cmp::Ordering::Less => {
            println!(" v{latest} available.");
        }
    }

    if check_only {
        println!("Run `agentis update` to install.");
        return Ok(());
    }

    // Find the right asset for this platform
    let asset_name = platform_asset_name()
        .ok_or_else(|| AgentisError::General("unsupported platform for self-update".into()))?;

    let assets = release
        .get("assets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AgentisError::General("no assets in release".into()))?;

    let download_url = assets
        .iter()
        .find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            if name == asset_name {
                asset
                    .get("browser_download_url")?
                    .as_str()
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            AgentisError::General(format!(
                "no binary for this platform ({asset_name}) in release v{latest}"
            ))
        })?;

    // Download the binary
    println!("Downloading {asset_name}...");
    let mut dl_response = ureq::get(&download_url)
        .header("User-Agent", "agentis-self-update")
        .call()
        .map_err(|e| AgentisError::General(format!("download failed: {e}")))?;

    let binary = dl_response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024) // 50 MB limit
        .read_to_vec()
        .map_err(|e| AgentisError::General(format!("failed to read binary: {e}")))?;

    if binary.is_empty() {
        return Err(AgentisError::General("downloaded empty file".into()));
    }

    println!("Downloaded {} bytes. Replacing binary...", binary.len());

    // Replace the current executable
    let current_exe = std::env::current_exe()
        .map_err(|e| AgentisError::General(format!("cannot locate current executable: {e}")))?;

    // Write downloaded binary to temp dir (always writable)
    let tmp_path = std::env::temp_dir().join("agentis-update-tmp");
    std::fs::write(&tmp_path, &binary)
        .map_err(|e| AgentisError::General(format!("failed to write temp file: {e}")))?;

    // Set executable permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| AgentisError::General(format!("failed to set permissions: {e}")))?;
    }

    // Replace the running binary.
    // On Linux, `cp` over a running binary fails with "Text file busy".
    // The fix: remove first (unlinks the inode while the old process keeps
    // running), then copy the new file into the now-free path.
    let needs_sudo = match std::fs::rename(&tmp_path, &current_exe) {
        Ok(_) => false,
        Err(_) => {
            // Try remove + copy (handles "Text file busy")
            let _ = std::fs::remove_file(&current_exe);
            match std::fs::copy(&tmp_path, &current_exe) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    false
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => true,
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(AgentisError::General(format!(
                        "failed to replace binary: {e}"
                    )));
                }
            }
        }
    };

    if needs_sudo {
        println!("Permission required \u{2014} re-running install with sudo...");
        // sudo rm + cp to handle both permission and "Text file busy"
        let _ = std::process::Command::new("sudo")
            .arg("rm")
            .arg("-f")
            .arg(&current_exe)
            .status();
        let status = std::process::Command::new("sudo")
            .arg("cp")
            .arg(&tmp_path)
            .arg(&current_exe)
            .status()
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp_path);
                AgentisError::General(format!("failed to run sudo: {e}"))
            })?;
        let _ = std::fs::remove_file(&tmp_path);
        if !status.success() {
            return Err(AgentisError::General(
                "sudo cp failed \u{2014} update aborted".into(),
            ));
        }
    }

    println!("Updated v{current} -> v{latest}.");
    Ok(())
}

/// Detect platform and return the GitHub release asset name.
fn platform_asset_name() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("agentis-linux-x86_64")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some("agentis-linux-aarch64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some("agentis-macos-x86_64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("agentis-macos-aarch64")
    } else {
        None
    }
}

/// Compare two semver strings (major.minor.patch).
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    };
    parse(a).cmp(&parse(b))
}

/// Silently check for updates (at most once per 24h). Print a hint if newer version exists.
/// Never blocks for more than 3 seconds. Errors are swallowed — this must never break normal usage.
fn maybe_notify_update() {
    let cache_path = match std::env::var("HOME") {
        Ok(home) => std::path::PathBuf::from(home).join(".agentis-update"),
        Err(_) => return,
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Read cache: line 1 = timestamp, line 2 = latest version
    let (cached_ts, cached_version) = read_update_cache(&cache_path);

    // Only hit the network once per day
    if now.saturating_sub(cached_ts) < 86400 {
        if !cached_version.is_empty() {
            show_update_hint(&cached_version);
        }
        return;
    }

    // Fetch latest version with a short timeout
    let latest = match fetch_latest_version_quiet() {
        Some(v) => v,
        None => return,
    };

    // Update cache (best-effort)
    let _ = std::fs::write(&cache_path, format!("{now}\n{latest}"));

    show_update_hint(&latest);
}

fn show_update_hint(latest: &str) {
    let current = env!("CARGO_PKG_VERSION");
    if compare_versions(current, latest) == std::cmp::Ordering::Less {
        eprintln!();
        eprintln!(
            "Update available: v{current} \u{2192} v{latest} \u{2014} run `agentis update` to install."
        );
    }
}

fn read_update_cache(path: &std::path::Path) -> (u64, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (0, String::new()),
    };
    let mut lines = content.lines();
    let ts: u64 = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
    let version = lines.next().unwrap_or("").to_string();
    (ts, version)
}

/// Fetch latest release version from GitHub with a 3-second timeout.
/// Returns None on any error (no network, timeout, parse failure).
fn fetch_latest_version_quiet() -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .build()
        .into();

    let mut response = agent
        .get("https://api.github.com/repos/Replikanti/agentis/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "agentis-self-update")
        .call()
        .ok()?;

    let body = response.body_mut().read_to_string().ok()?;
    let release = json::parse(&body).ok()?;
    let tag = release.get("tag_name")?.as_str()?;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_equal() {
        assert_eq!(
            compare_versions("0.6.2", "0.6.2"),
            std::cmp::Ordering::Equal
        );
    }

    #[test]
    fn version_compare_less_patch() {
        assert_eq!(compare_versions("0.6.1", "0.6.2"), std::cmp::Ordering::Less);
    }

    #[test]
    fn version_compare_less_minor() {
        assert_eq!(compare_versions("0.5.9", "0.6.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn version_compare_less_major() {
        assert_eq!(compare_versions("0.9.9", "1.0.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn version_compare_greater() {
        assert_eq!(
            compare_versions("1.0.0", "0.9.9"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn platform_asset_returns_some() {
        // Should return Some on any supported build target
        let name = platform_asset_name();
        if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
            assert!(name.is_some());
            assert!(name.unwrap().starts_with("agentis-"));
        }
    }

    #[test]
    fn update_cache_roundtrip() {
        let dir = std::env::temp_dir().join("agentis-test-cache");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("update-cache-test");
        std::fs::write(&path, "1700000000\n0.7.0").unwrap();
        let (ts, ver) = read_update_cache(&path);
        assert_eq!(ts, 1700000000);
        assert_eq!(ver, "0.7.0");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn update_cache_missing_file() {
        let path = std::path::Path::new("/tmp/agentis-nonexistent-cache-file");
        let (ts, ver) = read_update_cache(path);
        assert_eq!(ts, 0);
        assert_eq!(ver, "");
    }

    #[test]
    fn update_cache_corrupt() {
        let dir = std::env::temp_dir();
        let path = dir.join("agentis-test-cache-corrupt");
        std::fs::write(&path, "not-a-number\n").unwrap();
        let (ts, ver) = read_update_cache(&path);
        assert_eq!(ts, 0);
        assert_eq!(ver, "");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bundled_examples_parse() {
        for (name, content) in BUNDLED_EXAMPLES {
            if !name.ends_with(".ag") {
                continue;
            }
            let result = crate::parser::Parser::parse_source(content);
            assert!(result.is_ok(), "example {name} failed to parse: {result:?}");
        }
    }

    #[test]
    fn parse_size_bytes_units() {
        assert_eq!(parse_size_bytes("10MB"), Some(10 * 1024 * 1024));
        assert_eq!(parse_size_bytes("1GB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size_bytes("512KB"), Some(512 * 1024));
        assert_eq!(parse_size_bytes("100B"), Some(100));
        assert_eq!(parse_size_bytes("100"), Some(100));
        assert_eq!(parse_size_bytes("abc"), None);
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
