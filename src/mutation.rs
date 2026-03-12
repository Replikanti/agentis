// Mutation engine for agent evolution (Phase 7, M29).
//
// Generates agent variants by mutating prompt instruction strings.
// Uses the configured LLM backend for real mutations, or deterministic
// perturbations with mock backend. Source reconstruction via targeted
// string replacement of instruction literals.

use crate::ast::{AgentDecl, Declaration, Expr, Statement};
use crate::llm::LlmBackend;
use crate::parser::Parser;

// --- Agent info extracted from parsed source ---

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub name: String,
    pub instruction: String,
}

/// Extract all agent declarations and their prompt instructions from source.
pub fn extract_agents(source: &str) -> Result<Vec<AgentInfo>, String> {
    let program = Parser::parse_source(source).map_err(|e| format!("parse error: {e}"))?;
    let mut agents = Vec::new();
    for decl in &program.declarations {
        if let Declaration::Agent(agent) = decl {
            if let Some(instruction) = find_prompt_instruction_in_agent(agent) {
                agents.push(AgentInfo {
                    name: agent.name.clone(),
                    instruction,
                });
            }
        }
    }
    Ok(agents)
}

/// Find the first prompt instruction string in an agent's body.
fn find_prompt_instruction_in_agent(agent: &AgentDecl) -> Option<String> {
    for stmt in &agent.body.statements {
        if let Some(instr) = find_prompt_in_statement(stmt) {
            return Some(instr);
        }
    }
    None
}

fn find_prompt_in_statement(stmt: &Statement) -> Option<String> {
    match stmt {
        Statement::Let(l) => find_prompt_in_expr(&l.value),
        Statement::Return(ret) => ret.value.as_ref().and_then(find_prompt_in_expr),
        Statement::Expression(expr_stmt) => find_prompt_in_expr(&expr_stmt.expr),
        _ => None,
    }
}

fn find_prompt_in_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Prompt(p) => Some(p.instruction.clone()),
        Expr::If(if_expr) => {
            if let Some(i) = find_prompt_in_expr(&if_expr.condition) {
                return Some(i);
            }
            for s in &if_expr.then_block.statements {
                if let Some(i) = find_prompt_in_statement(s) {
                    return Some(i);
                }
            }
            if let Some(ref eb) = if_expr.else_block {
                for s in &eb.statements {
                    if let Some(i) = find_prompt_in_statement(s) {
                        return Some(i);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

// --- Mock perturbations ---

const MOCK_PERTURBATIONS: &[(&str, &str)] = &[
    ("Carefully ", ""),               // prepend
    ("", " Be precise."),             // append
    ("Step by step: ", ""),           // prepend
    ("", " Think twice."),            // append
    ("As an expert, ", ""),           // prepend
    ("", " Be thorough."),            // append
    ("Concisely ", ""),               // prepend
    ("", " Double-check your work."), // append
];

/// Apply a deterministic mock perturbation to an instruction.
pub fn mock_mutate(instruction: &str, index: usize) -> String {
    let (prefix, suffix) = MOCK_PERTURBATIONS[index % MOCK_PERTURBATIONS.len()];
    format!("{prefix}{instruction}{suffix}")
}

// --- LLM-based mutation ---

const DEFAULT_MUTATION_PROMPT: &str =
    "Rephrase this instruction differently while preserving its intent: ";

/// Mutate an instruction using the LLM backend.
pub fn llm_mutate(
    instruction: &str,
    backend: &dyn LlmBackend,
    custom_template: Option<&str>,
) -> Result<String, String> {
    let mutation_instruction = match custom_template {
        Some(template) => template.replace("{instruction}", instruction),
        None => format!("{DEFAULT_MUTATION_PROMPT}{instruction}"),
    };

    let type_ann = crate::ast::TypeAnnotation::Named("string".to_string());
    let result = backend
        .complete(&mutation_instruction, instruction, &type_ann, None)
        .map_err(|e| format!("LLM mutation failed: {e}"))?;

    // Extract the string value from the result
    match result {
        crate::evaluator::Value::String(s) => Ok(s),
        other => Ok(format!("{other}")),
    }
}

// --- Source reconstruction ---

/// Replace the first occurrence of a prompt instruction string literal in source.
/// Searches for `"<old_instruction>"` and replaces with `"<new_instruction>"`.
/// Returns the modified source, or None if the instruction was not found.
pub fn replace_instruction(
    source: &str,
    old_instruction: &str,
    new_instruction: &str,
) -> Option<String> {
    // Build the quoted string to search for.
    // We need to match the escaped form as it appears in source.
    let old_quoted = format!("\"{}\"", escape_for_source(old_instruction));
    let new_quoted = format!("\"{}\"", escape_for_source(new_instruction));

    // Find and replace the first occurrence only
    let pos = source.find(&old_quoted)?;
    let mut result = String::with_capacity(source.len() + new_quoted.len() - old_quoted.len());
    result.push_str(&source[..pos]);
    result.push_str(&new_quoted);
    result.push_str(&source[pos + old_quoted.len()..]);
    Some(result)
}

/// Escape a string for embedding in source code (minimal escaping).
fn escape_for_source(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c => result.push(c),
        }
    }
    result
}

// --- Mutation result ---

#[derive(Debug, Clone)]
pub struct MutationResult {
    pub filename: String,
    pub source: String,
    pub mutated_agents: Vec<String>,
}

/// Generate N mutated variants of a source file.
pub fn generate_variants(
    source: &str,
    base_name: &str,
    count: usize,
    agent_filter: Option<&str>,
    backend: &dyn LlmBackend,
    custom_template: Option<&str>,
) -> Result<Vec<MutationResult>, String> {
    let agents = extract_agents(source)?;
    if agents.is_empty() {
        return Err("no agents with prompt instructions found in source".to_string());
    }

    let eligible: Vec<&AgentInfo> = match agent_filter {
        Some(name) => {
            let filtered: Vec<&AgentInfo> = agents.iter().filter(|a| a.name == name).collect();
            if filtered.is_empty() {
                return Err(format!(
                    "agent '{}' not found. Available: {}",
                    name,
                    agents
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            filtered
        }
        None => agents.iter().collect(),
    };

    let is_mock = backend.name() == "mock";
    let mut variants = Vec::new();

    for i in 0..count {
        // Pick agent to mutate (round-robin through eligible)
        let agent = eligible[i % eligible.len()];

        // Generate mutation
        let new_instruction = if is_mock {
            mock_mutate(&agent.instruction, i)
        } else {
            llm_mutate(&agent.instruction, backend, custom_template)?
        };

        // Replace in source
        let new_source = replace_instruction(source, &agent.instruction, &new_instruction)
            .ok_or_else(|| {
                format!(
                    "could not find instruction literal for agent '{}'",
                    agent.name
                )
            })?;

        let filename = format!("{}-m{}.ag", base_name, i + 1);
        variants.push(MutationResult {
            filename,
            source: new_source,
            mutated_agents: vec![agent.name.clone()],
        });
    }

    Ok(variants)
}

/// Format a dry-run preview for a mutation.
pub fn format_dry_run(
    index: usize,
    total: usize,
    agent_name: &str,
    old_instruction: &str,
    new_instruction: &str,
) -> String {
    format!(
        "Variant {}/{}:\n  Agent: {}\n  Old:  \"{}\"\n  New:  \"{}\"",
        index + 1,
        total,
        agent_name,
        old_instruction,
        new_instruction
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_agents_from_classify() {
        let source = r#"
            type Category { label: string, confidence: float }
            agent classifier(text: string) -> Category {
                let result = prompt("Classify this text", text) -> Category;
                return result;
            }
            let r = classifier("test");
        "#;
        let agents = extract_agents(source).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "classifier");
        assert_eq!(agents[0].instruction, "Classify this text");
    }

    #[test]
    fn extract_multiple_agents() {
        let source = r#"
            agent a(x: string) -> string {
                let r = prompt("Do A", x) -> string;
                return r;
            }
            agent b(x: string) -> string {
                let r = prompt("Do B", x) -> string;
                return r;
            }
        "#;
        let agents = extract_agents(source).unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "a");
        assert_eq!(agents[1].name, "b");
    }

    #[test]
    fn extract_no_agents() {
        let source = "let x = 5;";
        let agents = extract_agents(source).unwrap();
        assert!(agents.is_empty());
    }

    #[test]
    fn mock_mutate_cycles() {
        let instr = "Do something";
        assert_eq!(mock_mutate(instr, 0), "Carefully Do something");
        assert_eq!(mock_mutate(instr, 1), "Do something Be precise.");
        assert_eq!(mock_mutate(instr, 2), "Step by step: Do something");
        assert_eq!(mock_mutate(instr, 8), "Carefully Do something"); // wraps
    }

    #[test]
    fn replace_instruction_basic() {
        let source = r#"let r = prompt("Hello world", x) -> string;"#;
        let result = replace_instruction(source, "Hello world", "Greetings planet").unwrap();
        assert_eq!(
            result,
            r#"let r = prompt("Greetings planet", x) -> string;"#
        );
    }

    #[test]
    fn replace_instruction_not_found() {
        let source = r#"let r = prompt("Hello", x) -> string;"#;
        assert!(replace_instruction(source, "Goodbye", "Test").is_none());
    }

    #[test]
    fn replace_instruction_preserves_rest() {
        let source = "// comment\nagent a(x: string) -> string {\n    let r = prompt(\"Do task\", x) -> string;\n    return r;\n}\nlet y = 5;";
        let result = replace_instruction(source, "Do task", "Carefully do task").unwrap();
        assert!(result.contains("\"Carefully do task\""));
        assert!(result.contains("// comment"));
        assert!(result.contains("let y = 5;"));
    }

    #[test]
    fn generate_variants_mock() {
        let source = r#"
            agent worker(x: string) -> string {
                let r = prompt("Process this", x) -> string;
                return r;
            }
            let y = worker("test");
        "#;
        let backend = crate::llm::MockBackend;
        let variants = generate_variants(source, "test", 3, None, &backend, None).unwrap();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].filename, "test-m1.ag");
        assert_eq!(variants[1].filename, "test-m2.ag");
        assert_eq!(variants[2].filename, "test-m3.ag");
        // Each variant should have different content
        assert!(variants[0].source.contains("Carefully Process this"));
        assert!(variants[1].source.contains("Process this Be precise."));
        assert!(variants[2].source.contains("Step by step: Process this"));
    }

    #[test]
    fn generate_variants_with_agent_filter() {
        let source = r#"
            agent a(x: string) -> string {
                let r = prompt("Do A", x) -> string;
                return r;
            }
            agent b(x: string) -> string {
                let r = prompt("Do B", x) -> string;
                return r;
            }
        "#;
        let backend = crate::llm::MockBackend;
        let variants = generate_variants(source, "test", 2, Some("b"), &backend, None).unwrap();
        assert_eq!(variants.len(), 2);
        // Only agent "b" should be mutated
        assert_eq!(variants[0].mutated_agents, vec!["b"]);
        assert!(variants[0].source.contains("\"Do A\"")); // agent a unchanged
        assert!(variants[0].source.contains("Carefully Do B")); // agent b mutated
    }

    #[test]
    fn generate_variants_bad_agent_filter() {
        let source = r#"
            agent a(x: string) -> string {
                let r = prompt("Do A", x) -> string;
                return r;
            }
        "#;
        let backend = crate::llm::MockBackend;
        let err =
            generate_variants(source, "test", 1, Some("nonexistent"), &backend, None).unwrap_err();
        assert!(err.contains("not found"));
        assert!(err.contains("Available: a"));
    }

    #[test]
    fn generate_variants_no_agents_error() {
        let source = "let x = 5;";
        let backend = crate::llm::MockBackend;
        let err = generate_variants(source, "test", 1, None, &backend, None).unwrap_err();
        assert!(err.contains("no agents"));
    }

    #[test]
    fn dry_run_format() {
        let text = format_dry_run(
            0,
            5,
            "classifier",
            "Classify text",
            "Step by step: Classify text",
        );
        assert!(text.contains("Variant 1/5:"));
        assert!(text.contains("Agent: classifier"));
        assert!(text.contains("Old:  \"Classify text\""));
        assert!(text.contains("New:  \"Step by step: Classify text\""));
    }

    #[test]
    fn escape_for_source_basic() {
        assert_eq!(escape_for_source("hello"), "hello");
        assert_eq!(escape_for_source("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_for_source("line\nnewline"), "line\\nnewline");
    }
}
