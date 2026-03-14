// Prompt cost instrumentation, budget prediction, and confidence scoring (Phase 13).
//
// M54: PromptRecord + PromptCostHistory — session-level cost tracking + JSONL persistence
// M55: estimate_cb() — budget prediction from historical data + heuristic fallback
// M56: compute_agreement(), majority_index() — ensemble agreement for confidence builtin
// M57: fuzzy_agreement(), spread_score(), ConfidenceReport — rich confidence metrics
// M58: format_stats(), merge_stats(), parse_stats_bytes() — CLI + bundle portability

use crate::json::{self, JsonValue};
use std::path::Path;

// --- Prompt Cost History (M54) ---

/// A single recorded prompt call.
#[derive(Debug, Clone)]
pub struct PromptRecord {
    pub instruction_hash: String,
    pub input_len: usize,
    pub cb_cost: u64,
    pub prompt_count: u64,
    pub backend: String,
}

/// Session-level collector + JSONL persistence for prompt cost data.
pub struct PromptCostHistory {
    records: Vec<PromptRecord>,
}

impl PromptCostHistory {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Load history from `.agentis/prompt_stats.jsonl`.
    pub fn load(root: &Path) -> Self {
        let path = root.join("prompt_stats.jsonl");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::new(),
        };
        parse_jsonl_records(&content)
    }

    /// Record a prompt call.
    pub fn record(&mut self, rec: PromptRecord) {
        self.records.push(rec);
    }

    /// Save to `.agentis/prompt_stats.jsonl`.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = root.join("prompt_stats.jsonl");
        let content = records_to_jsonl(&self.records);
        std::fs::write(&path, content)
    }

    /// Weighted average of prompt_count for records with similar input size (within 2× range).
    /// Recent records weighted 0.8, older records 0.2.
    pub fn avg_prompts_for_input_size(&self, len: usize) -> f64 {
        let lower = len / 2;
        let upper = len.saturating_mul(2);
        let matching: Vec<&PromptRecord> = self
            .records
            .iter()
            .filter(|r| r.input_len >= lower && r.input_len <= upper)
            .collect();
        if matching.is_empty() {
            return 0.0;
        }
        weighted_avg(&matching, |r| r.prompt_count as f64)
    }

    /// Weighted average of prompt_count for records matching exact instruction hash.
    pub fn avg_prompts_for_instruction(&self, hash: &str) -> Option<f64> {
        let matching: Vec<&PromptRecord> = self
            .records
            .iter()
            .filter(|r| r.instruction_hash == hash)
            .collect();
        if matching.is_empty() {
            return None;
        }
        Some(weighted_avg(&matching, |r| r.prompt_count as f64))
    }

    /// Filter by backend name.
    #[allow(dead_code)]
    pub fn filter_by_backend(&self, name: &str) -> Self {
        Self {
            records: self
                .records
                .iter()
                .filter(|r| r.backend == name)
                .cloned()
                .collect(),
        }
    }

    pub fn records(&self) -> &[PromptRecord] {
        &self.records
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

/// Weighted average with decay: last half of records get weight 0.8, first half get 0.2.
fn weighted_avg(records: &[&PromptRecord], f: impl Fn(&PromptRecord) -> f64) -> f64 {
    if records.is_empty() {
        return 0.0;
    }
    let mid = records.len() / 2;
    let mut sum = 0.0;
    let mut weight_sum = 0.0;
    for (i, rec) in records.iter().enumerate() {
        let w = if i >= mid { 0.8 } else { 0.2 };
        sum += f(rec) * w;
        weight_sum += w;
    }
    sum / weight_sum
}

fn records_to_jsonl(records: &[PromptRecord]) -> String {
    let mut content = String::new();
    for rec in records {
        let obj = json::object(vec![
            (
                "instruction_hash",
                JsonValue::String(rec.instruction_hash.clone()),
            ),
            ("input_len", JsonValue::Int(rec.input_len as i64)),
            ("cb_cost", JsonValue::Int(rec.cb_cost as i64)),
            ("prompt_count", JsonValue::Int(rec.prompt_count as i64)),
            ("backend", JsonValue::String(rec.backend.clone())),
        ]);
        content.push_str(&format!("{obj}\n"));
    }
    content
}

fn parse_jsonl_records(content: &str) -> PromptCostHistory {
    let mut records = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(parsed) = crate::json::parse(line)
            && let (Some(ih), Some(il), Some(cc), Some(pc), Some(be)) = (
                parsed.get("instruction_hash").and_then(|v| v.as_str()),
                parsed.get("input_len").and_then(|v| v.as_i64()),
                parsed.get("cb_cost").and_then(|v| v.as_i64()),
                parsed.get("prompt_count").and_then(|v| v.as_i64()),
                parsed.get("backend").and_then(|v| v.as_str()),
            )
        {
            records.push(PromptRecord {
                instruction_hash: ih.to_string(),
                input_len: il as usize,
                cb_cost: cc as u64,
                prompt_count: pc as u64,
                backend: be.to_string(),
            });
        }
    }
    PromptCostHistory { records }
}

// --- Budget Prediction (M55) ---

/// Hash an instruction string to a short hex hash (12 chars).
pub fn instruction_hash(instruction: &str) -> String {
    crate::audit::sha256_short(instruction)
}

/// Estimate CB cost for a prompt strategy.
pub fn estimate_cb(instruction: &str, input: &str, history: &PromptCostHistory) -> i64 {
    let instr_hash = instruction_hash(instruction);
    let input_len = input.len();

    // 1. Check historical avg for exact instruction hash
    if let Some(avg) = history.avg_prompts_for_instruction(&instr_hash) {
        return (avg * 50.0) as i64;
    }

    // 2. Check historical avg for similar input sizes
    let avg_by_size = history.avg_prompts_for_input_size(input_len);
    if avg_by_size > 0.0 {
        return (avg_by_size * 50.0) as i64;
    }

    // 3. Heuristic fallback
    heuristic_estimate(instruction, input_len)
}

fn heuristic_estimate(instruction: &str, input_len: usize) -> i64 {
    let instruction_complexity = instruction.split_whitespace().count() as f64 / 10.0;
    let estimate = (1.0 + input_len as f64 / 2000.0 + instruction_complexity) * 50.0 * 1.1;
    estimate as i64
}

// --- Agreement & Confidence (M56, M57) ---

/// Fraction of responses matching the majority response (exact string match).
pub fn compute_agreement(responses: &[String]) -> f64 {
    if responses.is_empty() {
        return 1.0;
    }
    let majority_idx = majority_index(responses);
    let majority = &responses[majority_idx];
    let count = responses.iter().filter(|r| *r == majority).count();
    count as f64 / responses.len() as f64
}

/// Index of the most common response.
pub fn majority_index(responses: &[String]) -> usize {
    if responses.is_empty() {
        return 0;
    }
    let mut best_idx = 0;
    let mut best_count = 0;
    for (i, resp) in responses.iter().enumerate() {
        let count = responses.iter().filter(|r| *r == resp).count();
        if count > best_count {
            best_count = count;
            best_idx = i;
        }
    }
    best_idx
}

/// Fuzzy agreement using normalized Levenshtein distance.
/// 1.0 = all identical, 0.0 = maximally different.
pub fn fuzzy_agreement(responses: &[String]) -> f64 {
    if responses.len() <= 1 {
        return 1.0;
    }
    let n = responses.len();
    let mut total_similarity = 0.0;
    let mut pair_count = 0;
    for i in 0..n {
        for j in (i + 1)..n {
            let dist = crate::library::levenshtein(&responses[i], &responses[j]);
            let max_len = responses[i].len().max(responses[j].len());
            let similarity = if max_len == 0 {
                1.0
            } else {
                1.0 - (dist as f64 / max_len as f64)
            };
            total_similarity += similarity;
            pair_count += 1;
        }
    }
    if pair_count == 0 {
        1.0
    } else {
        total_similarity / pair_count as f64
    }
}

/// Diversity score: 1.0 = all unique, 0.0 = all same.
pub fn spread_score(responses: &[String]) -> f64 {
    if responses.len() <= 1 {
        return 0.0;
    }
    let unique: std::collections::HashSet<&String> = responses.iter().collect();
    let n = responses.len() as f64;
    (unique.len() as f64 - 1.0) / (n - 1.0)
}

/// Confidence report returned by the `confidence` builtin.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConfidenceReport {
    pub value: String,
    pub confidence: f64,
    pub samples: usize,
    pub agreement: f64,
    pub spread: f64,
    pub all_responses: Vec<String>,
    pub backend_used: String,
}

// --- Stats formatting (M58) ---

/// Format prompt cost history as human-readable summary.
pub fn format_stats(history: &PromptCostHistory) -> String {
    if history.is_empty() {
        return "No prompt cost data recorded.\n".to_string();
    }
    let total = history.len();
    let avg_input: f64 =
        history.records().iter().map(|r| r.input_len as f64).sum::<f64>() / total as f64;
    let avg_cb: f64 =
        history.records().iter().map(|r| r.cb_cost as f64).sum::<f64>() / total as f64;
    let total_cb: u64 = history.records().iter().map(|r| r.cb_cost).sum();

    // Backend breakdown
    let mut backends: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for rec in history.records() {
        *backends.entry(rec.backend.as_str()).or_insert(0) += 1;
    }

    let mut out = String::new();
    out.push_str("Prompt Cost Statistics\n");
    out.push_str(&format!("  Total prompts:     {total}\n"));
    out.push_str(&format!("  Total CB spent:    {total_cb}\n"));
    out.push_str(&format!("  Avg input size:    {avg_input:.0} chars\n"));
    out.push_str(&format!("  Avg CB per prompt: {avg_cb:.1}\n"));
    out.push_str("  Backends:\n");
    for (name, count) in &backends {
        out.push_str(&format!("    {name}: {count}\n"));
    }
    out
}

/// Format prompt cost history as JSON.
pub fn format_stats_json(history: &PromptCostHistory) -> String {
    let total = history.len() as i64;
    let avg_input = if total > 0 {
        history.records().iter().map(|r| r.input_len as f64).sum::<f64>() / total as f64
    } else {
        0.0
    };
    let avg_cb = if total > 0 {
        history.records().iter().map(|r| r.cb_cost as f64).sum::<f64>() / total as f64
    } else {
        0.0
    };
    let total_cb: i64 = history.records().iter().map(|r| r.cb_cost as i64).sum();

    let obj = json::object(vec![
        ("total_prompts", JsonValue::Int(total)),
        ("total_cb", JsonValue::Int(total_cb)),
        ("avg_input_size", JsonValue::Float(avg_input)),
        ("avg_cb_per_prompt", JsonValue::Float(avg_cb)),
    ]);
    format!("{obj}")
}

// --- Deduplication for bundle import (M58) ---

/// Merge imported prompt stats into local stats with deduplication.
/// Dedup key: (instruction_hash, input_len_bucket, backend).
/// Input length bucketed to nearest 500 chars.
pub fn merge_stats(local: &mut PromptCostHistory, imported: &PromptCostHistory) {
    let bucket = |len: usize| -> usize { (len / 500) * 500 };

    let existing: std::collections::HashSet<(String, usize, String)> = local
        .records()
        .iter()
        .map(|r| {
            (
                r.instruction_hash.clone(),
                bucket(r.input_len),
                r.backend.clone(),
            )
        })
        .collect();

    for rec in imported.records() {
        let key = (
            rec.instruction_hash.clone(),
            bucket(rec.input_len),
            rec.backend.clone(),
        );
        if !existing.contains(&key) {
            local.record(rec.clone());
        }
    }
}

/// Format stats grouped by instruction hash, for `--per-identity` display.
pub fn format_stats_per_instruction(history: &PromptCostHistory) -> String {
    if history.is_empty() {
        return "No prompt cost data recorded.\n".to_string();
    }
    let mut groups: std::collections::BTreeMap<&str, (usize, u64, usize)> =
        std::collections::BTreeMap::new();
    for rec in history.records() {
        let entry = groups
            .entry(rec.instruction_hash.as_str())
            .or_insert((0, 0, 0));
        entry.0 += 1;
        entry.1 += rec.cb_cost;
        entry.2 += rec.input_len;
    }
    let mut out = String::new();
    out.push_str("Prompt Cost Statistics (per instruction)\n");
    out.push_str(&format!(
        "  {:>12}  {:>6}  {:>8}  {:>10}\n",
        "instr_hash", "calls", "total_cb", "avg_input"
    ));
    for (hash, (count, cb, input_sum)) in &groups {
        let avg_input = *input_sum as f64 / *count as f64;
        out.push_str(&format!(
            "  {:>12}  {:>6}  {:>8}  {:>10.0}\n",
            hash, count, cb, avg_input
        ));
    }
    out
}

/// Parse raw JSONL bytes into a PromptCostHistory.
pub fn parse_stats_bytes(data: &[u8]) -> PromptCostHistory {
    let content = String::from_utf8_lossy(data);
    parse_jsonl_records(&content)
}

/// Serialize history to JSONL bytes (for bundle).
#[allow(dead_code)]
pub fn stats_to_bytes(history: &PromptCostHistory) -> Vec<u8> {
    records_to_jsonl(history.records()).into_bytes()
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    // --- M54: Prompt Cost History ---

    #[test]
    fn empty_history() {
        let h = PromptCostHistory::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert_eq!(h.avg_prompts_for_input_size(100), 0.0);
        assert_eq!(h.avg_prompts_for_instruction("abc"), None);
    }

    #[test]
    fn record_and_avg_by_size() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "b".into(),
            input_len: 150,
            cb_cost: 50,
            prompt_count: 2,
            backend: "mock".into(),
        });
        let avg = h.avg_prompts_for_input_size(120);
        assert!(avg > 0.0);
    }

    #[test]
    fn avg_by_instruction_known() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "xyz".into(),
            input_len: 50,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "xyz".into(),
            input_len: 200,
            cb_cost: 50,
            prompt_count: 3,
            backend: "mock".into(),
        });
        let avg = h.avg_prompts_for_instruction("xyz").unwrap();
        assert!(avg > 0.0);
    }

    #[test]
    fn avg_by_instruction_unknown() {
        let h = PromptCostHistory::new();
        assert_eq!(h.avg_prompts_for_instruction("unknown"), None);
    }

    #[test]
    fn load_missing_file() {
        let h = PromptCostHistory::load(std::path::Path::new("/nonexistent"));
        assert!(h.is_empty());
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "abc123".into(),
            input_len: 42,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "def456".into(),
            input_len: 1000,
            cb_cost: 100,
            prompt_count: 2,
            backend: "http".into(),
        });
        h.save(root).unwrap();

        let loaded = PromptCostHistory::load(root);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.records()[0].instruction_hash, "abc123");
        assert_eq!(loaded.records()[0].input_len, 42);
        assert_eq!(loaded.records()[1].cb_cost, 100);
        assert_eq!(loaded.records()[1].backend, "http");
    }

    #[test]
    fn weighted_decay_favors_recent() {
        let mut h = PromptCostHistory::new();
        // Old records (first half): prompt_count = 1
        for _ in 0..10 {
            h.record(PromptRecord {
                instruction_hash: "x".into(),
                input_len: 100,
                cb_cost: 50,
                prompt_count: 1,
                backend: "mock".into(),
            });
        }
        // Recent records (second half): prompt_count = 5
        for _ in 0..10 {
            h.record(PromptRecord {
                instruction_hash: "x".into(),
                input_len: 100,
                cb_cost: 50,
                prompt_count: 5,
                backend: "mock".into(),
            });
        }
        let avg = h.avg_prompts_for_instruction("x").unwrap();
        // Should be closer to 5 than to 1 due to decay weighting
        assert!(avg > 3.0, "weighted avg {avg} should favor recent records");
    }

    #[test]
    fn filter_by_backend() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 10,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "b".into(),
            input_len: 20,
            cb_cost: 50,
            prompt_count: 1,
            backend: "http".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "c".into(),
            input_len: 30,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let mock_only = h.filter_by_backend("mock");
        assert_eq!(mock_only.len(), 2);
        let http_only = h.filter_by_backend("http");
        assert_eq!(http_only.len(), 1);
    }

    // --- M55: estimate_cb ---

    #[test]
    fn estimate_empty_history_small_input() {
        let h = PromptCostHistory::new();
        let est = estimate_cb("classify", "hello", &h);
        // Heuristic: (1 + 5/2000 + 1/10) * 50 * 1.1 ≈ 61
        assert!(est >= 50, "estimate {est} should be >= 50");
    }

    #[test]
    fn estimate_empty_history_large_input() {
        let h = PromptCostHistory::new();
        let large = "x".repeat(10000);
        let est = estimate_cb("analyze deeply", &large, &h);
        assert!(est > 50, "estimate {est} for large input should be > 50");
    }

    #[test]
    fn estimate_with_instruction_history() {
        let mut h = PromptCostHistory::new();
        let hash = instruction_hash("classify");
        h.record(PromptRecord {
            instruction_hash: hash.clone(),
            input_len: 50,
            cb_cost: 50,
            prompt_count: 2,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: hash,
            input_len: 60,
            cb_cost: 100,
            prompt_count: 2,
            backend: "mock".into(),
        });
        let est = estimate_cb("classify", "test", &h);
        assert_eq!(est, 100); // avg 2 * 50
    }

    #[test]
    fn estimate_with_size_history() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "other".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 3,
            backend: "mock".into(),
        });
        // "different" instruction so instruction match fails, but size matches
        let est = estimate_cb("different", &"x".repeat(120), &h);
        assert!(est > 50, "size-based estimate {est} should use history");
    }

    // --- M56: Agreement ---

    #[test]
    fn agreement_all_same() {
        let r = vec!["a".into(), "a".into(), "a".into()];
        assert_eq!(compute_agreement(&r), 1.0);
    }

    #[test]
    fn agreement_two_of_three() {
        let r = vec!["a".into(), "a".into(), "b".into()];
        let a = compute_agreement(&r);
        assert!((a - 2.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn agreement_all_different() {
        let r = vec!["a".into(), "b".into(), "c".into()];
        let a = compute_agreement(&r);
        assert!((a - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn agreement_empty() {
        assert_eq!(compute_agreement(&[]), 1.0);
    }

    #[test]
    fn majority_picks_most_common() {
        let r = vec!["a".into(), "b".into(), "a".into(), "c".into(), "a".into()];
        let idx = majority_index(&r);
        assert_eq!(r[idx], "a");
    }

    // --- M57: Fuzzy agreement & spread ---

    #[test]
    fn fuzzy_agreement_identical() {
        let r = vec!["hello".into(), "hello".into(), "hello".into()];
        assert_eq!(fuzzy_agreement(&r), 1.0);
    }

    #[test]
    fn fuzzy_agreement_nearly_identical() {
        let r = vec!["hello".into(), "hallo".into()];
        let a = fuzzy_agreement(&r);
        assert!(a > 0.7, "fuzzy agreement {a} for nearly identical strings");
    }

    #[test]
    fn fuzzy_agreement_different() {
        let r = vec!["abc".into(), "xyz".into()];
        let a = fuzzy_agreement(&r);
        assert!(a < 0.5, "fuzzy agreement {a} for different strings");
    }

    #[test]
    fn spread_all_same() {
        let r = vec!["a".into(), "a".into(), "a".into()];
        assert_eq!(spread_score(&r), 0.0);
    }

    #[test]
    fn spread_all_different() {
        let r = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(spread_score(&r), 1.0);
    }

    #[test]
    fn spread_single() {
        let r = vec!["a".into()];
        assert_eq!(spread_score(&r), 0.0);
    }

    // --- M58: Stats formatting ---

    #[test]
    fn format_stats_empty() {
        let h = PromptCostHistory::new();
        let s = format_stats(&h);
        assert!(s.contains("No prompt cost data"));
    }

    #[test]
    fn format_stats_populated() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "b".into(),
            input_len: 200,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let s = format_stats(&h);
        assert!(s.contains("Total prompts:     2"));
        assert!(s.contains("Total CB spent:    100"));
        assert!(s.contains("mock: 2"));
    }

    #[test]
    fn format_stats_json_valid() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let j = format_stats_json(&h);
        let parsed = crate::json::parse(&j).unwrap();
        assert_eq!(parsed.get("total_prompts").unwrap().as_i64(), Some(1));
        assert_eq!(parsed.get("total_cb").unwrap().as_i64(), Some(50));
    }

    // --- M58: Merge / dedup ---

    #[test]
    fn merge_deduplicates() {
        let mut local = PromptCostHistory::new();
        local.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let mut imported = PromptCostHistory::new();
        // Same bucket (100 rounds to 0 in 500-bucket)
        imported.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 120,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        // Different instruction
        imported.record(PromptRecord {
            instruction_hash: "b".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        merge_stats(&mut local, &imported);
        assert_eq!(local.len(), 2); // 1 original + 1 new (deduped)
    }

    #[test]
    fn repeated_import_no_duplicates() {
        let mut local = PromptCostHistory::new();
        local.record(PromptRecord {
            instruction_hash: "a".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let mut imported = PromptCostHistory::new();
        imported.record(PromptRecord {
            instruction_hash: "b".into(),
            input_len: 600,
            cb_cost: 50,
            prompt_count: 1,
            backend: "http".into(),
        });
        merge_stats(&mut local, &imported);
        assert_eq!(local.len(), 2);
        // Import again — should not add duplicates
        let imported2 = PromptCostHistory {
            records: vec![PromptRecord {
                instruction_hash: "b".into(),
                input_len: 600,
                cb_cost: 50,
                prompt_count: 1,
                backend: "http".into(),
            }],
        };
        merge_stats(&mut local, &imported2);
        assert_eq!(local.len(), 2); // no duplicates
    }

    #[test]
    fn stats_bytes_roundtrip() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "test".into(),
            input_len: 42,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        let bytes = stats_to_bytes(&h);
        let loaded = parse_stats_bytes(&bytes);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.records()[0].instruction_hash, "test");
    }

    #[test]
    fn stats_no_data_returns_message() {
        let dir = tempfile::tempdir().unwrap();
        let h = PromptCostHistory::load(dir.path());
        let s = format_stats(&h);
        assert!(s.contains("No prompt cost data"));
        let j = format_stats_json(&h);
        let parsed = crate::json::parse(&j).unwrap();
        assert_eq!(parsed.get("total_prompts").unwrap().as_i64(), Some(0));
    }

    #[test]
    fn format_stats_per_instruction_groups() {
        let mut h = PromptCostHistory::new();
        h.record(PromptRecord {
            instruction_hash: "aaa".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "aaa".into(),
            input_len: 200,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        h.record(PromptRecord {
            instruction_hash: "bbb".into(),
            input_len: 300,
            cb_cost: 100,
            prompt_count: 1,
            backend: "http".into(),
        });
        let s = format_stats_per_instruction(&h);
        assert!(s.contains("aaa"));
        assert!(s.contains("bbb"));
        assert!(s.contains("per instruction"));
    }
}
