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

// --- Simple PRNG ---

/// Simple xorshift64 PRNG — no external crate dependency.
pub struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    /// Create a new PRNG from a seed (must be non-zero for xorshift).
    pub fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    /// Random f64 in [0.0, 1.0).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() as f64) / (u64::MAX as f64)
    }

    /// Random usize in [0, bound).
    pub fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }
}

// --- Evolution config ---

#[allow(dead_code)]
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
    #[allow(dead_code)]
    pub source_hash: String,
    pub parent_hash: String,
    pub generation: usize,
    pub score: f64,
    #[allow(dead_code)]
    pub prompt_count: usize,
    pub mutations: Vec<String>,
    /// Outcome of this variant's evaluation (M45).
    #[allow(dead_code)]
    pub outcome: String,
    /// Wall-clock evaluation time in milliseconds (M45).
    #[allow(dead_code)]
    pub elapsed_ms: u64,
    /// CB spent during evaluation (M45).
    #[allow(dead_code)]
    pub cb_spent: u64,
}

// --- Ancestor record (M45) ---

/// A distilled record of an ancestor's outcome, exposed via `introspect.ancestor_failures`
/// and `introspect.ancestor_successes`.
#[derive(Debug, Clone)]
pub struct AncestorRecord {
    pub generation: usize,
    pub outcome: String,
    pub fitness_score: f64,
    pub code_hash: String,
    pub elapsed_ms: u64,
}

// --- Generation result ---

#[derive(Debug, Clone)]
pub struct GenResult {
    #[allow(dead_code)]
    pub generation: usize,
    #[allow(dead_code)]
    pub best_score: f64,
    #[allow(dead_code)]
    pub avg_score: f64,
    pub avg_prompts: f64,
    #[allow(dead_code)]
    pub variant_count: usize,
    #[allow(dead_code)]
    pub best_source: String,
    #[allow(dead_code)]
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
    pub provenance: String, // "seed-file", "population", or "library"
}

/// Hash source content using SHA-256 (reuses ObjectStore utility).
pub fn hash_source(source: &str) -> String {
    ObjectStore::hash_bytes(source.as_bytes())
}

#[allow(clippy::too_many_arguments)]
pub fn generate_tracked_variants(
    parents: &[(String, String)], // (source, source_hash) pairs
    population: usize,
    generation: usize,
    base_name: &str,
    agent_filter: Option<&str>,
    backend: &dyn crate::llm::LlmBackend,
    custom_template: Option<&str>,
    mock_offset: usize,
    default_provenance: &str,
    library_seeds: &[(String, String)],
    warm_start_prob: f64,
    rng: &mut SimpleRng,
) -> Result<Vec<TrackedVariant>, String> {
    let mut variants = Vec::new();
    let is_mock = backend.name() == "mock";

    for i in 0..population {
        // Decide parent: library seed (warm-start) or population
        let use_library =
            !library_seeds.is_empty() && warm_start_prob > 0.0 && rng.next_f64() < warm_start_prob;

        let (parent_source, parent_hash, provenance) = if use_library {
            let idx = rng.next_usize(library_seeds.len());
            (
                library_seeds[idx].0.as_str(),
                library_seeds[idx].1.as_str(),
                "library".to_string(),
            )
        } else {
            let parent_idx = i % parents.len();
            (
                parents[parent_idx].0.as_str(),
                parents[parent_idx].1.as_str(),
                default_provenance.to_string(),
            )
        };

        let agents = mutation::extract_agents(parent_source)?;
        if agents.is_empty() {
            return Err("no agents with prompt instructions found in source".to_string());
        }

        let eligible: Vec<&mutation::AgentInfo> = match agent_filter {
            Some(name) => agents.iter().filter(|a| a.name == name).collect(),
            None => agents.iter().collect(),
        };
        if eligible.is_empty() {
            return Err(format!(
                "agent filter '{}' matched no agents",
                agent_filter.unwrap_or("")
            ));
        }

        let agent = eligible[i % eligible.len()];
        let new_instruction = if is_mock {
            mutation::mock_mutate(&agent.instruction, mock_offset + i)
        } else {
            mutation::llm_mutate(&agent.instruction, backend, custom_template)?
        };

        let new_source =
            mutation::replace_instruction(parent_source, &agent.instruction, &new_instruction)
                .ok_or_else(|| {
                    format!(
                        "could not find instruction literal for agent '{}'",
                        agent.name
                    )
                })?;

        let new_hash = hash_source(&new_source);
        let filename = format!("{}-g{:02}-m{}.ag", base_name, generation, i + 1);

        variants.push(TrackedVariant {
            source: new_source,
            source_hash: new_hash,
            parent_hash: parent_hash.to_string(),
            filename,
            mutated_agents: vec![agent.name.clone()],
            provenance,
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
        let mutations_json: Vec<json::JsonValue> = variant
            .mutated_agents
            .iter()
            .map(|s| json::JsonValue::String(s.clone()))
            .collect();

        let mut fields: Vec<(&str, json::JsonValue)> = vec![
            ("ts", json::JsonValue::Int(ts as i64)),
            ("gen", json::JsonValue::Int(generation as i64)),
            (
                "source_hash",
                json::JsonValue::String(variant.source_hash.clone()),
            ),
            (
                "parent_hash",
                json::JsonValue::String(variant.parent_hash.clone()),
            ),
            ("score", json::JsonValue::Float(arena_entry.score)),
            (
                "prompt_count",
                json::JsonValue::Int(arena_entry.prompt_count as i64),
            ),
            ("mutations", json::JsonValue::Array(mutations_json)),
            (
                "provenance",
                json::JsonValue::String(variant.provenance.clone()),
            ),
            ("weights", json::JsonValue::String(weights.to_string())),
        ];

        if let Some(ms) = arena_entry.eval_time_ms {
            fields.push(("eval_time_ms", json::JsonValue::Int(ms as i64)));
        }

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
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(val) = crate::json::parse(line) {
                let source_hash = val
                    .get("source_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let parent_hash = val
                    .get("parent_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let generation = val.get("gen").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                let score = val.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let prompt_count = val
                    .get("prompt_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as usize;
                let mutations = val
                    .get("mutations")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                // M45: read outcome and eval_time_ms if present
                let outcome = val
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|e| {
                        if e.contains("CognitiveOverload") || e.contains("budget") {
                            "cb_exhausted".to_string()
                        } else if e.contains("timeout") || e.contains("Timeout") {
                            "timeout".to_string()
                        } else {
                            "validation_failed".to_string()
                        }
                    })
                    .unwrap_or_else(|| "survived".to_string());
                let elapsed_ms = val
                    .get("eval_time_ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as u64;

                if !source_hash.is_empty() {
                    map.insert(
                        source_hash.clone(),
                        LineageEntry {
                            source_hash,
                            parent_hash,
                            generation,
                            score,
                            prompt_count,
                            mutations,
                            outcome,
                            elapsed_ms,
                            cb_spent: 0,
                        },
                    );
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
                let mutation_suffix = entry
                    .mutations
                    .first()
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
    chain
        .iter()
        .map(|(label, score)| match score {
            Some(s) => format!("{} ({:.3})", label, s),
            None => label.clone(),
        })
        .collect::<Vec<_>>()
        .join(" → ")
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
    out.push_str(&format!(
        "  Est. prompt calls: {} ({} per eval × {} evals)\n",
        est_prompts, prompt_count, total_evaluations
    ));

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
                out.push_str(&format!(
                    "  Estimated time:    ~{:.0}s (local inference, $0)\n",
                    est_seconds
                ));
            } else {
                out.push_str(&format!(
                    "  Estimated time:    ~{:.1} min (local inference, $0)\n",
                    est_minutes
                ));
            }
        }
        "http" => {
            // Very rough: 1 token ≈ 4 chars, $0.003 per 1K input tokens (cheap model)
            let est_tokens = (avg_instruction_len / 2) * est_prompts;
            let est_cost = (est_tokens as f64 / 1000.0) * 0.003;
            out.push_str(&format!(
                "  Estimated cost:    ~${:.2} (approx {} tokens)\n",
                est_cost, est_tokens
            ));
        }
        _ => {
            out.push_str("  Estimated cost:    unknown backend\n");
        }
    }

    out
}

/// Count provenance types in tracked variants: (seed-file, population, library).
pub fn count_provenance(variants: &[TrackedVariant]) -> (usize, usize, usize) {
    let mut seed_file = 0;
    let mut population = 0;
    let mut library = 0;
    for v in variants {
        match v.provenance.as_str() {
            "seed-file" => seed_file += 1,
            "library" => library += 1,
            _ => population += 1,
        }
    }
    (seed_file, population, library)
}

// --- Event Hooks (M42) ---

/// Evolution event that can trigger hooks.
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    Stagnation,
    NewBest,
    ValidationFail,
    Crash,
}

/// Action to execute when a hook fires.
#[derive(Debug, Clone, PartialEq)]
pub enum HookAction {
    ReduceBudget(f64),
    InjectLibrary(usize),
    Checkpoint,
    Tag(String),
    LibAdd,
    Log(String),
    Skip,
}

/// A parsed hook: event → list of actions.
#[derive(Debug, Clone)]
pub struct Hook {
    pub event: HookEvent,
    pub actions: Vec<HookAction>,
}

/// Parse hooks from config. Returns hooks + any parse errors.
/// Config keys: `hooks.on_stagnation`, `hooks.on_new_best`,
/// `hooks.on_validation_fail`, `hooks.on_crash`.
pub fn parse_hooks(cfg: &crate::config::Config) -> Result<Vec<Hook>, String> {
    let entries = [
        ("hooks.on_stagnation", HookEvent::Stagnation),
        ("hooks.on_new_best", HookEvent::NewBest),
        ("hooks.on_validation_fail", HookEvent::ValidationFail),
        ("hooks.on_crash", HookEvent::Crash),
    ];

    let mut hooks = Vec::new();
    for (key, event) in &entries {
        if let Some(value) = cfg.get(key) {
            let actions =
                parse_hook_actions(value).map_err(|e| format!("invalid hook '{}': {}", key, e))?;
            if !actions.is_empty() {
                hooks.push(Hook {
                    event: event.clone(),
                    actions,
                });
            }
        }
    }
    Ok(hooks)
}

/// Parse a space-separated action string into actions.
fn parse_hook_actions(value: &str) -> Result<Vec<HookAction>, String> {
    let tokens: Vec<&str> = value.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok(vec![]);
    }

    let mut actions = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match tokens[i] {
            "checkpoint" => {
                actions.push(HookAction::Checkpoint);
                i += 1;
            }
            "lib_add" => {
                actions.push(HookAction::LibAdd);
                i += 1;
            }
            "skip" => {
                actions.push(HookAction::Skip);
                i += 1;
            }
            "reduce_budget" => {
                i += 1;
                let frac: f64 = tokens
                    .get(i)
                    .ok_or("reduce_budget requires a fraction argument")?
                    .parse()
                    .map_err(|_| "reduce_budget fraction must be a number")?;
                actions.push(HookAction::ReduceBudget(frac));
                i += 1;
            }
            "inject_library" => {
                i += 1;
                let count: usize = tokens
                    .get(i)
                    .ok_or("inject_library requires a count argument")?
                    .parse()
                    .map_err(|_| "inject_library count must be a number")?;
                actions.push(HookAction::InjectLibrary(count));
                i += 1;
            }
            "log" => {
                // Rest of tokens are the message
                let msg = tokens[i + 1..].join(" ");
                if msg.is_empty() {
                    return Err("log requires a message".to_string());
                }
                actions.push(HookAction::Log(msg));
                break; // log consumes rest of line
            }
            t if t.starts_with("tag=") => {
                let name = t.strip_prefix("tag=").unwrap();
                if name.is_empty() {
                    return Err("tag= requires a name".to_string());
                }
                actions.push(HookAction::Tag(name.to_string()));
                i += 1;
            }
            other => {
                return Err(format!("unknown action '{other}'"));
            }
        }
    }
    Ok(actions)
}

/// Find hooks matching a given event.
pub fn hooks_for_event<'a>(hooks: &'a [Hook], event: &HookEvent) -> Vec<&'a Hook> {
    hooks.iter().filter(|h| &h.event == event).collect()
}

// --- Adaptive Budget Manager (M41) ---

/// Per-lineage budget tracking.
#[derive(Debug, Clone)]
pub struct LineageBudget {
    pub seed_hash: String,
    pub allocated_fraction: f64,
    pub cumulative_cb: u64,
    pub recent_scores: Vec<f64>,
    pub stall_count: u32,
    pub active: bool,
}

/// Configuration for adaptive budget allocation.
pub struct AdaptiveBudgetConfig {
    pub window_size: usize,
    pub max_fraction: f64,
    pub min_improvement: f64,
}

impl Default for AdaptiveBudgetConfig {
    fn default() -> Self {
        Self {
            window_size: 5,
            max_fraction: 0.5,
            min_improvement: 0.01,
        }
    }
}

/// Manages per-lineage budget allocation across an evolution run.
pub struct AdaptiveBudgetManager {
    lineages: Vec<LineageBudget>,
    window_size: usize,
    max_fraction: f64,
    min_improvement: f64,
}

impl AdaptiveBudgetManager {
    pub fn new(config: AdaptiveBudgetConfig) -> Self {
        Self {
            lineages: Vec::new(),
            window_size: config.window_size,
            max_fraction: config.max_fraction,
            min_improvement: config.min_improvement,
        }
    }

    /// Register a new lineage (idempotent).
    pub fn register_lineage(&mut self, seed_hash: &str) {
        if self.lineages.iter().any(|l| l.seed_hash == seed_hash) {
            return;
        }
        self.lineages.push(LineageBudget {
            seed_hash: seed_hash.to_string(),
            allocated_fraction: 1.0, // normalize will set equal shares
            cumulative_cb: 0,
            recent_scores: Vec::new(),
            stall_count: 0,
            active: true,
        });
        self.normalize();
    }

    /// Update a lineage after a generation with its best score and CB spent.
    pub fn update(&mut self, seed_hash: &str, gen_best: f64, cb_spent: u64) {
        let active_count = self.active_count();
        let Some(lineage) = self.lineages.iter_mut().find(|l| l.seed_hash == seed_hash) else {
            return;
        };
        if !lineage.active {
            return;
        }

        lineage.cumulative_cb += cb_spent;
        lineage.recent_scores.push(gen_best);
        if lineage.recent_scores.len() > self.window_size {
            lineage.recent_scores.remove(0);
        }

        // Only adjust allocation once the window is full
        if lineage.recent_scores.len() >= self.window_size {
            let first = lineage.recent_scores[0];
            let last = *lineage.recent_scores.last().unwrap();
            let delta = last - first;

            if delta > self.min_improvement {
                // Growing: increase by 50%, capped at max_fraction
                lineage.allocated_fraction =
                    (lineage.allocated_fraction * 1.5).min(self.max_fraction);
                lineage.stall_count = 0;
            } else {
                // Stalled: reduce to 1/3
                lineage.stall_count += 1;
                lineage.allocated_fraction /= 3.0;
            }
        }

        // Terminate dead lineage (stall > 2 * window_size), keep at least 1
        let hard_cap = (self.window_size * 2) as u32;
        if lineage.stall_count > hard_cap && active_count > 1 {
            lineage.active = false;
            lineage.allocated_fraction = 0.0;
        }

        self.normalize();
    }

    /// Normalize active lineage fractions to sum to 1.0.
    fn normalize(&mut self) {
        let total: f64 = self
            .lineages
            .iter()
            .filter(|l| l.active)
            .map(|l| l.allocated_fraction)
            .sum();
        let active_count = self.active_count();

        if active_count == 0 {
            return;
        }

        if total == 0.0 {
            let equal = 1.0 / active_count as f64;
            for l in self.lineages.iter_mut().filter(|l| l.active) {
                l.allocated_fraction = equal;
            }
        } else {
            for l in self.lineages.iter_mut().filter(|l| l.active) {
                l.allocated_fraction /= total;
            }
        }
    }

    /// Allocate variant slots to lineages based on current fractions.
    /// Uses floor + remainder distribution to highest-fraction lineages.
    pub fn allocate_slots(&self, total_slots: usize) -> Vec<(String, usize)> {
        let active: Vec<&LineageBudget> = self.lineages.iter().filter(|l| l.active).collect();
        if active.is_empty() {
            return vec![];
        }

        let mut result: Vec<(String, usize)> = active
            .iter()
            .map(|l| {
                (
                    l.seed_hash.clone(),
                    (l.allocated_fraction * total_slots as f64).floor() as usize,
                )
            })
            .collect();

        let assigned: usize = result.iter().map(|(_, s)| s).sum();
        let mut remaining = total_slots.saturating_sub(assigned);

        // Distribute remainders by fractional part (descending)
        let mut fracs: Vec<(usize, f64)> = active
            .iter()
            .enumerate()
            .map(|(i, l)| {
                (
                    i,
                    l.allocated_fraction * total_slots as f64 - result[i].1 as f64,
                )
            })
            .collect();
        fracs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (idx, _) in fracs {
            if remaining == 0 {
                break;
            }
            result[idx].1 += 1;
            remaining -= 1;
        }

        result
    }

    #[allow(dead_code)]
    pub fn is_active(&self, seed_hash: &str) -> bool {
        self.lineages
            .iter()
            .any(|l| l.seed_hash == seed_hash && l.active)
    }

    pub fn active_count(&self) -> usize {
        self.lineages.iter().filter(|l| l.active).count()
    }

    /// Reduce all active lineage fractions by a multiplicative factor.
    /// E.g., `reduce_all(0.3)` reduces each fraction by 30%.
    pub fn reduce_all(&mut self, factor: f64) {
        for l in &mut self.lineages {
            if l.active {
                l.allocated_fraction *= 1.0 - factor;
            }
        }
        self.normalize();
    }

    /// Format allocation report for generation summary.
    pub fn format_report(&self, total_slots: usize) -> String {
        let allocation = self.allocate_slots(total_slots);
        allocation
            .iter()
            .map(|(hash, slots)| {
                let prefix = &hash[..8.min(hash.len())];
                format!("{prefix} {slots}/{total_slots}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Get lineage info for termination messages.
    pub fn terminated_lineages(&self) -> Vec<&LineageBudget> {
        self.lineages.iter().filter(|l| !l.active).collect()
    }

    /// Get the last score for a lineage (for termination messages).
    pub fn last_score(&self, seed_hash: &str) -> Option<f64> {
        self.lineages
            .iter()
            .find(|l| l.seed_hash == seed_hash)
            .and_then(|l| l.recent_scores.last().copied())
    }
}

/// Expand parents list according to adaptive budget slot allocation.
/// Groups parents by lineage, then repeats each group's parents to fill allocated slots.
pub fn expand_parents_by_allocation(
    parents: &[(String, String)],
    allocation: &[(String, usize)],
    lineage_map: &HashMap<String, String>,
    default_lineage: &str,
) -> Vec<(String, String)> {
    // Group parents by lineage
    let mut by_lineage: HashMap<String, Vec<&(String, String)>> = HashMap::new();
    for p in parents {
        let lineage = lineage_map
            .get(&p.1)
            .map(|s| s.as_str())
            .unwrap_or(default_lineage);
        by_lineage.entry(lineage.to_string()).or_default().push(p);
    }

    let mut result = Vec::new();
    for (lineage, slots) in allocation {
        if let Some(lp) = by_lineage.get(lineage) {
            for i in 0..*slots {
                result.push(lp[i % lp.len()].clone());
            }
        }
    }

    // If allocation produced fewer than expected (some lineages had no parents),
    // fill with any available parent
    if result.is_empty() && !parents.is_empty() {
        result.push(parents[0].clone());
    }

    result
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
        let mut rng = SimpleRng::new(42);
        let variants = generate_tracked_variants(
            &parents,
            3,
            1,
            "test",
            None,
            &backend,
            None,
            0,
            "seed-file",
            &[],
            0.0,
            &mut rng,
        )
        .unwrap();
        assert_eq!(variants.len(), 3);
        assert!(variants[0].source.contains("Carefully Process this"));
        assert!(!variants[0].parent_hash.is_empty());
        assert_eq!(variants[0].filename, "test-g01-m1.ag");
        assert_eq!(variants[0].provenance, "seed-file");
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
        let mut rng = SimpleRng::new(42);
        let variants = generate_tracked_variants(
            &parents,
            4,
            2,
            "test",
            None,
            &backend,
            None,
            0,
            "population",
            &[],
            0.0,
            &mut rng,
        )
        .unwrap();
        assert_eq!(variants.len(), 4);
        // Variants 0,2 from parent A, variants 1,3 from parent B
        assert_eq!(variants[0].parent_hash, hash_source(source_a));
        assert_eq!(variants[1].parent_hash, hash_source(source_b));
    }

    #[test]
    fn trace_lineage_simple() {
        let mut lineage = HashMap::new();
        lineage.insert(
            "hash_g1".to_string(),
            LineageEntry {
                source_hash: "hash_g1".to_string(),
                parent_hash: "hash_seed".to_string(),
                generation: 1,
                score: 0.72,
                prompt_count: 3,
                mutations: vec!["classifier".to_string()],
                outcome: "survived".to_string(),
                elapsed_ms: 0,
                cb_spent: 0,
            },
        );
        lineage.insert(
            "hash_g2".to_string(),
            LineageEntry {
                source_hash: "hash_g2".to_string(),
                parent_hash: "hash_g1".to_string(),
                generation: 2,
                score: 0.85,
                prompt_count: 2,
                mutations: vec!["classifier".to_string()],
                outcome: "survived".to_string(),
                elapsed_ms: 0,
                cb_spent: 0,
            },
        );

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
            provenance: "population".to_string(),
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
            worker: None,
            eval_time_ms: None,
        };

        write_generation_jsonl(
            &dir,
            1,
            &[(variant, arena_entry)],
            &FitnessWeights::default(),
        )
        .unwrap();

        let lineage = load_lineage(&dir);
        assert!(lineage.contains_key("abc123"));
        let entry = &lineage["abc123"];
        assert_eq!(entry.generation, 1);
        assert_eq!(entry.parent_hash, "parent000");
        assert!((entry.score - 0.85).abs() < 0.001);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn simple_rng_bounded() {
        let mut rng = SimpleRng::new(123);
        for _ in 0..1000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn simple_rng_usize_bounded() {
        let mut rng = SimpleRng::new(7);
        for _ in 0..100 {
            let v = rng.next_usize(5);
            assert!(v < 5, "out of bounds: {v}");
        }
    }

    #[test]
    fn simple_rng_zero_seed() {
        let mut rng = SimpleRng::new(0);
        // Should not panic or produce only zeros
        let v = rng.next_f64();
        assert!(v >= 0.0);
    }

    #[test]
    fn warm_start_provenance_all_library() {
        let source = r#"
            agent worker(x: string) -> string {
                let r = prompt("Process this", x) -> string;
                return r;
            }
            let y = worker("test");
        "#;
        let hash = hash_source(source);
        let parents = vec![(source.to_string(), hash)];
        let lib_source = r#"
            agent worker(x: string) -> string {
                let r = prompt("Library process", x) -> string;
                return r;
            }
            let y = worker("test");
        "#;
        let lib_hash = hash_source(lib_source);
        let lib_seeds = vec![(lib_source.to_string(), lib_hash)];
        let backend = crate::llm::MockBackend;
        let mut rng = SimpleRng::new(42);
        let variants = generate_tracked_variants(
            &parents,
            4,
            1,
            "test",
            None,
            &backend,
            None,
            0,
            "seed-file",
            &lib_seeds,
            1.0,
            &mut rng,
        )
        .unwrap();
        assert_eq!(variants.len(), 4);
        for v in &variants {
            assert_eq!(v.provenance, "library");
        }
    }

    #[test]
    fn no_warm_start_provenance_seed_file() {
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
        let mut rng = SimpleRng::new(42);
        let variants = generate_tracked_variants(
            &parents,
            3,
            1,
            "test",
            None,
            &backend,
            None,
            0,
            "seed-file",
            &[],
            0.0,
            &mut rng,
        )
        .unwrap();
        for v in &variants {
            assert_eq!(v.provenance, "seed-file");
        }
    }

    #[test]
    fn count_provenance_mixed() {
        let make = |prov: &str| TrackedVariant {
            source: String::new(),
            source_hash: String::new(),
            parent_hash: String::new(),
            filename: String::new(),
            mutated_agents: vec![],
            provenance: prov.to_string(),
        };
        let variants = vec![
            make("seed-file"),
            make("library"),
            make("population"),
            make("library"),
        ];
        let (s, p, l) = count_provenance(&variants);
        assert_eq!(s, 1);
        assert_eq!(p, 1);
        assert_eq!(l, 2);
    }

    #[test]
    fn jsonl_includes_provenance() {
        let dir = std::env::temp_dir().join(format!("agentis_prov_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let variant = TrackedVariant {
            source: "test".to_string(),
            source_hash: "hash1".to_string(),
            parent_hash: "parent1".to_string(),
            filename: "t.ag".to_string(),
            mutated_agents: vec!["w".to_string()],
            provenance: "library".to_string(),
        };
        let entry = ArenaEntry {
            file: "t.ag".to_string(),
            score: 0.5,
            cb_eff: 0.9,
            val_rate: 1.0,
            exp_rate: 0.0,
            prompt_count: 1,
            error: None,
            rounds: 1,
            worker: None,
            eval_time_ms: None,
        };

        write_generation_jsonl(&dir, 1, &[(variant, entry)], &FitnessWeights::default()).unwrap();

        let content = std::fs::read_to_string(dir.join("g01.jsonl")).unwrap();
        assert!(content.contains("\"provenance\":\"library\""));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Adaptive Budget Manager tests ---

    #[test]
    fn budget_single_lineage_gets_all_slots() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig::default());
        mgr.register_lineage("seed_a");
        let alloc = mgr.allocate_slots(8);
        assert_eq!(alloc.len(), 1);
        assert_eq!(alloc[0].1, 8);
    }

    #[test]
    fn budget_two_lineages_equal_initially() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig::default());
        mgr.register_lineage("seed_a");
        mgr.register_lineage("seed_b");
        let alloc = mgr.allocate_slots(8);
        assert_eq!(alloc.len(), 2);
        assert_eq!(alloc[0].1, 4);
        assert_eq!(alloc[1].1, 4);
    }

    #[test]
    fn budget_growing_lineage_gets_more() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig {
            window_size: 3,
            max_fraction: 0.8,
            min_improvement: 0.01,
        });
        mgr.register_lineage("grow");
        mgr.register_lineage("stall");

        // Feed growing scores to "grow", flat scores to "stall"
        for i in 0..3 {
            mgr.update("grow", 0.5 + (i as f64) * 0.1, 100);
            mgr.update("stall", 0.5, 100);
        }

        let alloc = mgr.allocate_slots(8);
        let grow_slots = alloc.iter().find(|(h, _)| h == "grow").unwrap().1;
        let stall_slots = alloc.iter().find(|(h, _)| h == "stall").unwrap().1;
        assert!(
            grow_slots > stall_slots,
            "growing lineage should get more slots: grow={grow_slots}, stall={stall_slots}"
        );
    }

    #[test]
    fn budget_stalled_lineage_reduced() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig {
            window_size: 3,
            max_fraction: 0.8,
            min_improvement: 0.01,
        });
        mgr.register_lineage("active");
        mgr.register_lineage("stalled");

        // Both stall, but keep active
        for _ in 0..3 {
            mgr.update("active", 0.5, 100);
            mgr.update("stalled", 0.5, 100);
        }

        // Now one grows, the other stays stalled
        for i in 0..3 {
            mgr.update("active", 0.6 + (i as f64) * 0.05, 100);
            mgr.update("stalled", 0.5, 100);
        }

        let alloc = mgr.allocate_slots(8);
        let active_slots = alloc.iter().find(|(h, _)| h == "active").unwrap().1;
        let stalled_slots = alloc.iter().find(|(h, _)| h == "stalled").unwrap().1;
        assert!(
            active_slots > stalled_slots,
            "active={active_slots}, stalled={stalled_slots}"
        );
    }

    #[test]
    fn budget_termination() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig {
            window_size: 2,
            max_fraction: 0.8,
            min_improvement: 0.01,
        });
        mgr.register_lineage("survivor");
        mgr.register_lineage("doomed");

        // Hard cap = window_size * 2 = 4
        // Survivor improves, doomed stays flat → doomed terminates
        for i in 0..10 {
            mgr.update("survivor", 0.5 + (i as f64) * 0.02, 100);
            mgr.update("doomed", 0.3, 100);
        }

        assert!(mgr.is_active("survivor"));
        assert!(!mgr.is_active("doomed"), "doomed should be terminated");

        let alloc = mgr.allocate_slots(8);
        assert_eq!(alloc.len(), 1);
        assert_eq!(alloc[0].0, "survivor");
        assert_eq!(alloc[0].1, 8);
    }

    #[test]
    fn budget_last_lineage_not_terminated() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig {
            window_size: 2,
            max_fraction: 0.8,
            min_improvement: 0.01,
        });
        mgr.register_lineage("only");

        for _ in 0..10 {
            mgr.update("only", 0.5, 100);
        }

        assert!(mgr.is_active("only"), "last lineage must not be terminated");
    }

    #[test]
    fn budget_slot_rounding() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig::default());
        mgr.register_lineage("a");
        mgr.register_lineage("b");
        mgr.register_lineage("c");

        // 3 lineages, 10 slots: 3.33 each → 3+3+3 = 9, remainder 1 goes to highest frac
        let alloc = mgr.allocate_slots(10);
        let total: usize = alloc.iter().map(|(_, s)| s).sum();
        assert_eq!(total, 10, "all slots must be allocated");
    }

    #[test]
    fn budget_normalization() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig::default());
        mgr.register_lineage("x");
        mgr.register_lineage("y");

        let total: f64 = mgr
            .lineages
            .iter()
            .filter(|l| l.active)
            .map(|l| l.allocated_fraction)
            .sum();
        assert!((total - 1.0).abs() < 0.001, "fractions should sum to 1.0");
    }

    #[test]
    fn budget_format_report() {
        let mut mgr = AdaptiveBudgetManager::new(AdaptiveBudgetConfig::default());
        mgr.register_lineage("abcdef1234567890");
        mgr.register_lineage("12345678abcdef90");

        let report = mgr.format_report(8);
        assert!(report.contains("abcdef12 4/8"));
        assert!(report.contains("12345678 4/8"));
    }

    #[test]
    fn expand_parents_basic() {
        let parents = vec![
            ("src_a".to_string(), "hash_a".to_string()),
            ("src_b".to_string(), "hash_b".to_string()),
        ];
        let mut lineage_map = HashMap::new();
        lineage_map.insert("hash_a".to_string(), "lineage_1".to_string());
        lineage_map.insert("hash_b".to_string(), "lineage_2".to_string());

        let allocation = vec![("lineage_1".to_string(), 3), ("lineage_2".to_string(), 1)];

        let expanded =
            expand_parents_by_allocation(&parents, &allocation, &lineage_map, "lineage_1");
        assert_eq!(expanded.len(), 4);
        // First 3 from lineage_1, last 1 from lineage_2
        assert_eq!(expanded[0].1, "hash_a");
        assert_eq!(expanded[1].1, "hash_a");
        assert_eq!(expanded[2].1, "hash_a");
        assert_eq!(expanded[3].1, "hash_b");
    }

    // --- Hook parsing tests ---

    #[test]
    fn parse_hooks_empty_config() {
        let cfg = crate::config::Config::parse("");
        let hooks = parse_hooks(&cfg).unwrap();
        assert!(hooks.is_empty());
    }

    #[test]
    fn parse_hooks_checkpoint() {
        let cfg = crate::config::Config::parse("hooks.on_crash = checkpoint");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::Crash);
        assert_eq!(hooks[0].actions, vec![HookAction::Checkpoint]);
    }

    #[test]
    fn parse_hooks_checkpoint_with_tag() {
        let cfg = crate::config::Config::parse("hooks.on_new_best = checkpoint tag=improved");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].event, HookEvent::NewBest);
        assert_eq!(
            hooks[0].actions,
            vec![
                HookAction::Checkpoint,
                HookAction::Tag("improved".to_string())
            ]
        );
    }

    #[test]
    fn parse_hooks_reduce_budget() {
        let cfg = crate::config::Config::parse("hooks.on_stagnation = reduce_budget 0.3");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks[0].event, HookEvent::Stagnation);
        assert_eq!(hooks[0].actions, vec![HookAction::ReduceBudget(0.3)]);
    }

    #[test]
    fn parse_hooks_inject_library() {
        let cfg = crate::config::Config::parse("hooks.on_stagnation = inject_library 3");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks[0].actions, vec![HookAction::InjectLibrary(3)]);
    }

    #[test]
    fn parse_hooks_log_message() {
        let cfg = crate::config::Config::parse("hooks.on_crash = log evolution crashed!");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(
            hooks[0].actions,
            vec![HookAction::Log("evolution crashed!".to_string())]
        );
    }

    #[test]
    fn parse_hooks_lib_add() {
        let cfg = crate::config::Config::parse("hooks.on_new_best = lib_add");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks[0].actions, vec![HookAction::LibAdd]);
    }

    #[test]
    fn parse_hooks_skip() {
        let cfg = crate::config::Config::parse("hooks.on_crash = skip");
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks[0].actions, vec![HookAction::Skip]);
    }

    #[test]
    fn parse_hooks_multiple_events() {
        let cfg = crate::config::Config::parse(
            "hooks.on_stagnation = reduce_budget 0.5\nhooks.on_new_best = checkpoint tag=best",
        );
        let hooks = parse_hooks(&cfg).unwrap();
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn parse_hooks_invalid_action() {
        let cfg = crate::config::Config::parse("hooks.on_crash = explode");
        let result = parse_hooks(&cfg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown action"));
    }

    #[test]
    fn parse_hooks_missing_argument() {
        let cfg = crate::config::Config::parse("hooks.on_stagnation = reduce_budget");
        let result = parse_hooks(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn hooks_for_event_filters() {
        let hooks = vec![
            Hook {
                event: HookEvent::Crash,
                actions: vec![HookAction::Checkpoint],
            },
            Hook {
                event: HookEvent::NewBest,
                actions: vec![HookAction::LibAdd],
            },
        ];
        let crash_hooks = hooks_for_event(&hooks, &HookEvent::Crash);
        assert_eq!(crash_hooks.len(), 1);
        assert_eq!(crash_hooks[0].event, HookEvent::Crash);

        let stag_hooks = hooks_for_event(&hooks, &HookEvent::Stagnation);
        assert!(stag_hooks.is_empty());
    }
}
