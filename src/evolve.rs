// Evolution loop for agent evolution (Phase 7, M30).
//
// Ties together mutation engine, arena runner, and fitness scoring
// into a generational evolutionary loop: mutate → arena → select → repeat.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::arena::ArenaEntry;
use crate::fitness::FitnessWeights;
use crate::json;
use crate::mutation;
use crate::storage::ObjectStore;

// --- Evolution config ---

#[derive(Debug, Clone)]
pub struct EvolveConfig {
    pub generations: usize,
    pub population: usize,
    pub agent_filter: Option<String>,
    pub custom_template: Option<String>,
    pub weights: FitnessWeights,
    pub budget_cap: Option<u64>,
    pub stop_on_stall: Option<usize>,
    pub show_lineage: bool,
    pub out_dir: PathBuf,
}

// --- Lineage entry ---

#[derive(Debug, Clone)]
pub struct LineageEntry {
    pub source_hash: String,
    pub parent_hash: String,
    pub generation: usize,
    pub score: f64,
    pub prompt_count: usize,
    pub mutations: Vec<String>,
}

// --- Generation result ---

#[derive(Debug, Clone)]
pub struct GenResult {
    pub generation: usize,
    pub best_score: f64,
    pub avg_score: f64,
    pub avg_prompts: f64,
    pub variant_count: usize,
    pub best_source: String,
    pub best_hash: String,
}

// --- Variant with source tracking ---

#[derive(Debug, Clone)]
pub struct TrackedVariant {
    pub source: String,
    pub source_hash: String,
    pub parent_hash: String,
    pub filename: String,
    pub mutated_agents: Vec<String>,
}

/// Hash source content using SHA-256 (reuses ObjectStore utility).
pub fn hash_source(source: &str) -> String {
    ObjectStore::hash_bytes(source.as_bytes())
}

/// Generate tracked variants from a set of parent sources.
/// Distributes mutations round-robin across parents.
pub fn generate_tracked_variants(
    parents: &[(String, String)], // (source, source_hash) pairs
    population: usize,
    generation: usize,
    base_name: &str,
    agent_filter: Option<&str>,
    backend: &dyn crate::llm::LlmBackend,
    custom_template: Option<&str>,
    mock_offset: usize,
) -> Result<Vec<TrackedVariant>, String> {
    let mut variants = Vec::new();
    let is_mock = backend.name() == "mock";

    for i in 0..population {
        // Pick parent round-robin
        let parent_idx = i % parents.len();
        let (parent_source, parent_hash) = &parents[parent_idx];

        let agents = mutation::extract_agents(parent_source)?;
        if agents.is_empty() {
            return Err("no agents with prompt instructions found in source".to_string());
        }

        let eligible: Vec<&mutation::AgentInfo> = match agent_filter {
            Some(name) => agents.iter().filter(|a| a.name == name).collect(),
            None => agents.iter().collect(),
        };
        if eligible.is_empty() {
            return Err(format!("agent filter '{}' matched no agents", agent_filter.unwrap_or("")));
        }

        let agent = eligible[i % eligible.len()];
        let new_instruction = if is_mock {
            mutation::mock_mutate(&agent.instruction, mock_offset + i)
        } else {
            mutation::llm_mutate(&agent.instruction, backend, custom_template)?
        };

        let new_source = mutation::replace_instruction(parent_source, &agent.instruction, &new_instruction)
            .ok_or_else(|| format!("could not find instruction literal for agent '{}'", agent.name))?;

        let new_hash = hash_source(&new_source);
        let filename = format!("{}-g{:02}-m{}.ag", base_name, generation, i + 1);

        variants.push(TrackedVariant {
            source: new_source,
            source_hash: new_hash,
            parent_hash: parent_hash.clone(),
            filename,
            mutated_agents: vec![agent.name.clone()],
        });
    }

    Ok(variants)
}

/// Write a per-generation JSONL file.
pub fn write_generation_jsonl(
    fitness_dir: &Path,
    generation: usize,
    entries: &[(TrackedVariant, ArenaEntry)],
    weights: &FitnessWeights,
) -> std::io::Result<()> {
    std::fs::create_dir_all(fitness_dir)?;
    let path = fitness_dir.join(format!("g{generation:02}.jsonl"));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for (variant, arena_entry) in entries {
        let mutations_json: Vec<json::JsonValue> = variant.mutated_agents.iter()
            .map(|s| json::JsonValue::String(s.clone()))
            .collect();

        let mut fields: Vec<(&str, json::JsonValue)> = vec![
            ("ts", json::JsonValue::Int(ts as i64)),
            ("gen", json::JsonValue::Int(generation as i64)),
            ("source_hash", json::JsonValue::String(variant.source_hash.clone())),
            ("parent_hash", json::JsonValue::String(variant.parent_hash.clone())),
            ("score", json::JsonValue::Float(arena_entry.score)),
            ("prompt_count", json::JsonValue::Int(arena_entry.prompt_count as i64)),
            ("mutations", json::JsonValue::Array(mutations_json)),
            ("weights", json::JsonValue::String(weights.to_string())),
        ];

        if let Some(ref e) = arena_entry.error {
            fields.push(("error", json::JsonValue::String(e.clone())));
        }

        let obj = json::object(fields);
        writeln!(file, "{obj}")?;
    }

    Ok(())
}

// --- Lineage loading ---

/// Load lineage data from per-generation JSONL files.
pub fn load_lineage(fitness_dir: &Path) -> HashMap<String, LineageEntry> {
    let mut map = HashMap::new();

    let entries = match std::fs::read_dir(fitness_dir) {
        Ok(e) => e,
        Err(_) => return map,
    };

    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension().is_some_and(|ext| ext == "jsonl")
                && e.file_name().to_string_lossy().starts_with('g')
        })
        .collect();
    files.sort_by_key(|e| e.file_name().to_string_lossy().to_string());

    for entry in &files {
        let content = match std::fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for line in content.lines() {
            if line.trim().is_empty() { continue; }
            if let Ok(val) = crate::json::parse(line) {
                let source_hash = val.get("source_hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let parent_hash = val.get("parent_hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let generation = val.get("gen").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                let score = val.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let prompt_count = val.get("prompt_count").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                let mutations = val.get("mutations")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();

                if !source_hash.is_empty() {
                    map.insert(source_hash.clone(), LineageEntry {
                        source_hash,
                        parent_hash,
                        generation,
                        score,
                        prompt_count,
                        mutations,
                    });
                }
            }
        }
    }

    map
}

/// Trace lineage from a source hash back to the seed.
/// Returns chain from seed to target: [(label, score)].
pub fn trace_lineage(
    lineage: &HashMap<String, LineageEntry>,
    target_hash: &str,
    seed_name: &str,
) -> Vec<(String, Option<f64>)> {
    let mut chain = Vec::new();
    let mut current = target_hash.to_string();

    // Walk backwards
    loop {
        if let Some(entry) = lineage.get(&current) {
            let label = if entry.generation == 0 {
                seed_name.to_string()
            } else {
                let mutation_suffix = entry.mutations.first()
                    .map(|m| format!(" [{}]", m))
                    .unwrap_or_default();
                format!("g{}{}", entry.generation, mutation_suffix)
            };
            chain.push((label, Some(entry.score)));
            if entry.parent_hash.is_empty() || entry.parent_hash == current {
                break;
            }
            current = entry.parent_hash.clone();
        } else {
            // Reached the seed (not in lineage map)
            chain.push((format!("{} (seed)", seed_name), None));
            break;
        }
    }

    chain.reverse();
    chain
}

/// Format lineage chain as a human-readable string.
pub fn format_lineage(chain: &[(String, Option<f64>)]) -> String {
    chain.iter().map(|(label, score)| {
        match score {
            Some(s) => format!("{} ({:.3})", label, s),
            None => label.clone(),
        }
    }).collect::<Vec<_>>().join(" → ")
}

/// Format dry-run cost estimation.
pub fn format_dry_run(
    generations: usize,
    population: usize,
    prompt_count: usize,
    backend_name: &str,
    avg_instruction_len: usize,
    tokens_per_second: f64,
) -> String {
    let total_mutations = population * generations;
    let total_evaluations = population * generations;
    let est_prompts = prompt_count * total_evaluations;

    let mut out = String::new();
    out.push_str("Dry-run estimation:\n");
    out.push_str(&format!("  Generations:       {}\n", generations));
    out.push_str(&format!("  Population:        {}\n", population));
    out.push_str(&format!("  Total mutations:   {}\n", total_mutations));
    out.push_str(&format!("  Total evaluations: {}\n", total_evaluations));
    out.push_str(&format!("  Est. prompt calls: {} ({} per eval × {} evals)\n",
        est_prompts, prompt_count, total_evaluations));

    match backend_name {
        "mock" => {
            out.push_str("  Estimated cost:    $0 (mock mode)\n");
        }
        "cli" => {
            // Estimate time for CLI/Ollama: avg_instruction_len chars ≈ tokens/4
            // Each prompt call: instruction + input. Rough: 2 * avg_instruction_len / 4 tokens
            let est_tokens = (avg_instruction_len / 2) * est_prompts;
            let est_seconds = est_tokens as f64 / tokens_per_second;
            let est_minutes = est_seconds / 60.0;
            if est_minutes < 1.0 {
                out.push_str(&format!("  Estimated time:    ~{:.0}s (local inference, $0)\n", est_seconds));
            } else {
                out.push_str(&format!("  Estimated time:    ~{:.1} min (local inference, $0)\n", est_minutes));
            }
        }
        "http" => {
            // Very rough: 1 token ≈ 4 chars, $0.003 per 1K input tokens (cheap model)
            let est_tokens = (avg_instruction_len / 2) * est_prompts;
            let est_cost = (est_tokens as f64 / 1000.0) * 0.003;
            out.push_str(&format!("  Estimated cost:    ~${:.2} (approx {} tokens)\n", est_cost, est_tokens));
        }
        _ => {
            out.push_str("  Estimated cost:    unknown backend\n");
        }
    }

    out
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_source_deterministic() {
        let h1 = hash_source("hello world");
        let h2 = hash_source("hello world");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn hash_source_different() {
        let h1 = hash_source("hello");
        let h2 = hash_source("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn generate_tracked_variants_mock() {
        let source = r#"
            agent worker(x: string) -> string {
                let r = prompt("Process this", x) -> string;
                return r;
            }
            let y = worker("test");
        "#;
        let hash = hash_source(source);
        let parents = vec![(source.to_string(), hash)];
        let backend = crate::llm::MockBackend;
        let variants = generate_tracked_variants(
            &parents, 3, 1, "test", None, &backend, None, 0,
        ).unwrap();
        assert_eq!(variants.len(), 3);
        assert!(variants[0].source.contains("Carefully Process this"));
        assert!(!variants[0].parent_hash.is_empty());
        assert_eq!(variants[0].filename, "test-g01-m1.ag");
    }

    #[test]
    fn generate_tracked_variants_from_multiple_parents() {
        let source_a = r#"
            agent a(x: string) -> string {
                let r = prompt("Do A", x) -> string;
                return r;
            }
            let y = a("test");
        "#;
        let source_b = r#"
            agent a(x: string) -> string {
                let r = prompt("Carefully Do A", x) -> string;
                return r;
            }
            let y = a("test");
        "#;
        let parents = vec![
            (source_a.to_string(), hash_source(source_a)),
            (source_b.to_string(), hash_source(source_b)),
        ];
        let backend = crate::llm::MockBackend;
        let variants = generate_tracked_variants(
            &parents, 4, 2, "test", None, &backend, None, 0,
        ).unwrap();
        assert_eq!(variants.len(), 4);
        // Variants 0,2 from parent A, variants 1,3 from parent B
        assert_eq!(variants[0].parent_hash, hash_source(source_a));
        assert_eq!(variants[1].parent_hash, hash_source(source_b));
    }

    #[test]
    fn trace_lineage_simple() {
        let mut lineage = HashMap::new();
        lineage.insert("hash_g1".to_string(), LineageEntry {
            source_hash: "hash_g1".to_string(),
            parent_hash: "hash_seed".to_string(),
            generation: 1,
            score: 0.72,
            prompt_count: 3,
            mutations: vec!["classifier".to_string()],
        });
        lineage.insert("hash_g2".to_string(), LineageEntry {
            source_hash: "hash_g2".to_string(),
            parent_hash: "hash_g1".to_string(),
            generation: 2,
            score: 0.85,
            prompt_count: 2,
            mutations: vec!["classifier".to_string()],
        });

        let chain = trace_lineage(&lineage, "hash_g2", "classify.ag");
        assert_eq!(chain.len(), 3); // seed + g1 + g2
        assert!(chain[0].0.contains("seed"));
        assert!(chain[1].0.contains("g1"));
        assert!(chain[2].0.contains("g2"));
    }

    #[test]
    fn format_lineage_chain() {
        let chain = vec![
            ("classify.ag (seed)".to_string(), None),
            ("g1 [classifier]".to_string(), Some(0.72)),
            ("g2 [classifier]".to_string(), Some(0.85)),
        ];
        let s = format_lineage(&chain);
        assert!(s.contains("classify.ag (seed)"));
        assert!(s.contains("→"));
        assert!(s.contains("g1 [classifier] (0.720)"));
        assert!(s.contains("g2 [classifier] (0.850)"));
    }

    #[test]
    fn dry_run_mock() {
        let s = format_dry_run(10, 8, 3, "mock", 40, 30.0);
        assert!(s.contains("Total mutations:   80"));
        assert!(s.contains("Est. prompt calls: 240"));
        assert!(s.contains("$0 (mock mode)"));
    }

    #[test]
    fn dry_run_cli() {
        let s = format_dry_run(5, 4, 2, "cli", 50, 30.0);
        assert!(s.contains("Total mutations:   20"));
        assert!(s.contains("local inference"));
    }

    #[test]
    fn dry_run_http() {
        let s = format_dry_run(5, 4, 2, "http", 50, 30.0);
        assert!(s.contains("Estimated cost:"));
        assert!(s.contains("$"));
    }

    #[test]
    fn write_and_load_lineage() {
        let dir = std::env::temp_dir().join(format!("agentis_evolve_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let variant = TrackedVariant {
            source: "test source".to_string(),
            source_hash: "abc123".to_string(),
            parent_hash: "parent000".to_string(),
            filename: "test-g01-m1.ag".to_string(),
            mutated_agents: vec!["worker".to_string()],
        };
        let arena_entry = ArenaEntry {
            file: "test-g01-m1.ag".to_string(),
            score: 0.85,
            cb_eff: 0.95,
            val_rate: 1.0,
            exp_rate: 0.5,
            prompt_count: 3,
            error: None,
            rounds: 1,
        };

        write_generation_jsonl(&dir, 1, &[(variant, arena_entry)], &FitnessWeights::default()).unwrap();

        let lineage = load_lineage(&dir);
        assert!(lineage.contains_key("abc123"));
        let entry = &lineage["abc123"];
        assert_eq!(entry.generation, 1);
        assert_eq!(entry.parent_hash, "parent000");
        assert!((entry.score - 0.85).abs() < 0.001);

        std::fs::remove_dir_all(&dir).ok();
    }
}
