mod ast;
mod evaluator;
mod lexer;
mod parser;
mod refs;
mod storage;

use std::path::Path;
use std::process;

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
        "branch" => {
            if args.len() < 3 {
                cmd_list_branches()
            } else {
                cmd_create_branch(&args[2])
            }
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
    eprintln!("  init                 Initialize a new Agentis repository");
    eprintln!("  commit <file>        Parse source file, store AST, update current branch");
    eprintln!("  run <branch>         Execute code from a branch's root hash");
    eprintln!("  branch [name]        List branches or create a new one");
    eprintln!("  log [branch]         Show commit log for a branch");
    eprintln!("  version              Show version");
}

fn agentis_root() -> std::path::PathBuf {
    Path::new(".agentis").to_path_buf()
}

fn ensure_initialized() -> Result<(ObjectStore, Refs), String> {
    let root = agentis_root();
    if !root.exists() {
        return Err("Not an Agentis repository. Run 'agentis init' first.".to_string());
    }
    Ok((ObjectStore::new(&root), Refs::new(&root)))
}

fn cmd_init() -> Result<(), String> {
    let root = agentis_root();
    if root.exists() {
        return Err("Agentis repository already initialized.".to_string());
    }
    ObjectStore::init(&root).map_err(|e| e.to_string())?;
    let refs = Refs::new(&root);
    refs.init().map_err(|e| e.to_string())?;
    println!("Initialized empty Agentis repository with genesis branch.");
    Ok(())
}

fn cmd_commit(source_file: &str) -> Result<(), String> {
    let (store, refs) = ensure_initialized()?;

    let source = std::fs::read_to_string(source_file)
        .map_err(|e| format!("cannot read '{source_file}': {e}"))?;

    let program = Parser::parse_source(&source)
        .map_err(|e| format!("parse error: {e}"))?;

    let hash = store.save(&program).map_err(|e| e.to_string())?;
    let branch = refs.commit(&hash).map_err(|e| e.to_string())?;

    println!("[{branch}] {hash}");
    Ok(())
}

fn cmd_run(branch: &str) -> Result<(), String> {
    let (store, refs) = ensure_initialized()?;

    let hash = refs.get_branch_hash(branch)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("branch '{branch}' has no commits"))?;

    let program: ast::Program = store.load(&hash).map_err(|e| e.to_string())?;

    let mut evaluator = Evaluator::new(DEFAULT_BUDGET);
    match evaluator.eval_program(&program) {
        Ok(_) => {
            for line in evaluator.output() {
                println!("{line}");
            }
            Ok(())
        }
        Err(e) => Err(format!("runtime error: {e}")),
    }
}

fn cmd_list_branches() -> Result<(), String> {
    let (_, refs) = ensure_initialized()?;

    let branches = refs.list_branches().map_err(|e| e.to_string())?;
    for (name, is_current) in &branches {
        if *is_current {
            println!("* {name}");
        } else {
            println!("  {name}");
        }
    }
    Ok(())
}

fn cmd_create_branch(name: &str) -> Result<(), String> {
    let (_, refs) = ensure_initialized()?;
    refs.create_branch(name, None).map_err(|e| e.to_string())?;
    println!("Created branch '{name}'.");
    Ok(())
}

fn cmd_log(branch: Option<&str>) -> Result<(), String> {
    let (_, refs) = ensure_initialized()?;

    let branch_name = match branch {
        Some(b) => b.to_string(),
        None => refs.current_branch().map_err(|e| e.to_string())?,
    };

    let log = refs.log(&branch_name).map_err(|e| e.to_string())?;
    if log.is_empty() {
        println!("No commits on branch '{branch_name}'.");
    } else {
        for hash in &log {
            println!("{hash}  ({branch_name})");
        }
    }
    Ok(())
}
