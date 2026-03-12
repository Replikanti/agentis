// Arena runner for agent evolution (Phase 7, M28).
//
// Runs multiple program variants side by side, ranks by fitness,
// reports standings. Variants run sequentially (not parallel).

use crate::fitness::{FitnessReport, FitnessWeights};
use crate::json;

// --- Arena result for a single variant ---

#[derive(Debug, Clone)]
pub struct ArenaEntry {
    pub file: String,
    pub score: f64,
    pub cb_eff: f64,
    pub val_rate: f64,
    pub exp_rate: f64,
    pub prompt_count: usize,
    pub error: Option<String>,
    pub rounds: usize,
    /// Which worker evaluated this variant (None = local).
    pub worker: Option<String>,
    /// Evaluation wall-clock time in milliseconds.
    pub eval_time_ms: Option<u64>,
}

impl ArenaEntry {
    /// Create from a single fitness report.
    pub fn from_report(file: &str, report: &FitnessReport, weights: &FitnessWeights) -> Self {
        Self {
            file: file.to_string(),
            score: report.score_with(weights),
            cb_eff: report.cb_efficiency(),
            val_rate: report.validate_rate(),
            exp_rate: report.explore_rate(),
            prompt_count: report.prompt_count,
            error: if report.error {
                Some("runtime error".to_string())
            } else {
                None
            },
            rounds: 1,
            worker: None,
            eval_time_ms: None,
        }
    }

    /// Create from an error (parse failure, etc.).
    pub fn from_error(file: &str, error: &str) -> Self {
        Self {
            file: file.to_string(),
            score: 0.0,
            cb_eff: 0.0,
            val_rate: 0.0,
            exp_rate: 0.0,
            prompt_count: 0,
            error: Some(truncate_error(error, 80)),
            rounds: 1,
            worker: None,
            eval_time_ms: None,
        }
    }

    /// Average multiple entries for the same file (multi-round).
    pub fn average(entries: &[ArenaEntry]) -> ArenaEntry {
        assert!(!entries.is_empty());
        let n = entries.len() as f64;
        let errors: Vec<_> = entries.iter().filter_map(|e| e.error.as_ref()).collect();
        let has_error = !errors.is_empty();

        // If all rounds errored, report error
        if errors.len() == entries.len() {
            return ArenaEntry {
                file: entries[0].file.clone(),
                score: 0.0,
                cb_eff: 0.0,
                val_rate: 0.0,
                exp_rate: 0.0,
                prompt_count: 0,
                error: Some(truncate_error(errors[0], 80)),
                rounds: entries.len(),
                worker: entries[0].worker.clone(),
                eval_time_ms: None,
            };
        }

        // Average only successful runs
        let successful: Vec<_> = entries.iter().filter(|e| e.error.is_none()).collect();
        let sn = successful.len() as f64;

        ArenaEntry {
            file: entries[0].file.clone(),
            score: successful.iter().map(|e| e.score).sum::<f64>() / sn,
            cb_eff: successful.iter().map(|e| e.cb_eff).sum::<f64>() / sn,
            val_rate: successful.iter().map(|e| e.val_rate).sum::<f64>() / sn,
            exp_rate: successful.iter().map(|e| e.exp_rate).sum::<f64>() / sn,
            prompt_count: (successful.iter().map(|e| e.prompt_count).sum::<usize>() as f64 / sn)
                .round() as usize,
            error: if has_error {
                Some(format!("{}/{} rounds failed", errors.len(), entries.len()))
            } else {
                None
            },
            rounds: entries.len(),
            worker: entries[0].worker.clone(),
            eval_time_ms: None,
        }
    }
}

fn truncate_error(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}

// --- Formatting ---

/// Format the arena results as a human-readable table.
pub fn format_table(entries: &[ArenaEntry], rounds: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Arena: {} variants, {} round{} each\n\n",
        entries.len(),
        rounds,
        if rounds == 1 { "" } else { "s" },
    ));

    // Find max file name length for column alignment
    let max_file_len = entries
        .iter()
        .map(|e| e.file.len())
        .max()
        .unwrap_or(4)
        .max(4)
        .min(30);

    out.push_str(&format!(
        "{:<6}{:<width$}  {:<8}{:<8}{:<7}{}\n",
        "RANK",
        "FILE",
        "SCORE",
        "CB_EFF",
        "VAL",
        "EXP",
        width = max_file_len + 2,
    ));

    for (i, entry) in entries.iter().enumerate() {
        let file_display = if entry.file.len() > 30 {
            format!("...{}", &entry.file[entry.file.len() - 27..])
        } else {
            entry.file.clone()
        };

        if let Some(ref err) = entry.error {
            out.push_str(&format!(
                "{:<6}{:<width$}  {:<8}\u{2014}       \u{2014}      \u{2014} (error: {})\n",
                i + 1,
                file_display,
                format!("{:.3}", entry.score),
                err,
                width = max_file_len + 2,
            ));
        } else {
            out.push_str(&format!(
                "{:<6}{:<width$}  {:<8}{:<8}{:<7}{:.2}\n",
                i + 1,
                file_display,
                format!("{:.3}", entry.score),
                format!("{:.2}", entry.cb_eff),
                format!("{:.2}", entry.val_rate),
                entry.exp_rate,
                width = max_file_len + 2,
            ));
        }
    }

    if let Some(winner) = entries.first() {
        if winner.error.is_none() {
            out.push_str(&format!(
                "\nWinner: {} (score: {:.3})\n",
                winner.file, winner.score
            ));
        }
    }

    out
}

/// Format a colony stats summary line.
pub fn format_colony_stats(entries: &[ArenaEntry], worker_count: usize) -> String {
    let local_count = entries
        .iter()
        .filter(|e| e.worker.as_deref() == Some("local"))
        .count();
    let eval_times: Vec<u64> = entries.iter().filter_map(|e| e.eval_time_ms).collect();
    let avg_ms = if eval_times.is_empty() {
        0
    } else {
        eval_times.iter().sum::<u64>() / eval_times.len() as u64
    };
    format!(
        "Colony: {} worker{}, {} local fallback{}, avg eval {}ms",
        worker_count,
        if worker_count == 1 { "" } else { "s" },
        local_count,
        if local_count == 1 { "" } else { "s" },
        avg_ms,
    )
}

/// Format the arena results as JSON.
pub fn format_json(entries: &[ArenaEntry], rounds: usize) -> String {
    let items: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut fields: Vec<(&str, json::JsonValue)> = vec![
                ("rank", json::JsonValue::Int((i + 1) as i64)),
                ("file", json::JsonValue::String(e.file.clone())),
                ("score", json::JsonValue::Float(e.score)),
            ];

            if e.error.is_none() {
                fields.push(("cb_eff", json::JsonValue::Float(e.cb_eff)));
                fields.push(("val_rate", json::JsonValue::Float(e.val_rate)));
                fields.push(("exp_rate", json::JsonValue::Float(e.exp_rate)));
                fields.push(("prompt_count", json::JsonValue::Int(e.prompt_count as i64)));
                fields.push(("error", json::JsonValue::Null));
            } else {
                fields.push(("cb_eff", json::JsonValue::Null));
                fields.push(("val_rate", json::JsonValue::Null));
                fields.push(("exp_rate", json::JsonValue::Null));
                fields.push(("prompt_count", json::JsonValue::Int(e.prompt_count as i64)));
                fields.push(("error", json::JsonValue::String(e.error.clone().unwrap())));
            }

            if rounds > 1 {
                fields.push(("rounds", json::JsonValue::Int(rounds as i64)));
                fields.push(("rounds_avg", json::JsonValue::Bool(true)));
            }

            // Colony fields (only present when running with --workers)
            if let Some(ref w) = e.worker {
                fields.push(("worker", json::JsonValue::String(w.clone())));
            }
            if let Some(t) = e.eval_time_ms {
                fields.push(("eval_time_ms", json::JsonValue::Int(t as i64)));
            }

            format!("{}", json::object(fields))
        })
        .collect();

    format!("[{}]", items.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fitness::FitnessReport;

    fn make_report(
        cb_remaining: u64,
        val_passed: usize,
        val_total: usize,
        exp_passed: usize,
        exp_total: usize,
    ) -> FitnessReport {
        FitnessReport {
            cb_initial: 10000,
            cb_remaining,
            validates_passed: val_passed,
            validates_total: val_total,
            explores_passed: exp_passed,
            explores_total: exp_total,
            prompt_count: 3,
            error: false,
        }
    }

    #[test]
    fn entry_from_report() {
        let r = make_report(9500, 3, 3, 1, 2);
        let w = FitnessWeights::default();
        let entry = ArenaEntry::from_report("test.ag", &r, &w);
        assert_eq!(entry.file, "test.ag");
        assert!((entry.score - 0.885).abs() < 0.001);
        assert!(entry.error.is_none());
        assert_eq!(entry.rounds, 1);
    }

    #[test]
    fn entry_from_error() {
        let entry = ArenaEntry::from_error("bad.ag", "CognitiveOverload");
        assert_eq!(entry.score, 0.0);
        assert!(entry.error.is_some());
        assert!(entry.error.unwrap().contains("CognitiveOverload"));
    }

    #[test]
    fn entry_average_all_success() {
        let r1 = make_report(9500, 3, 3, 1, 2);
        let r2 = make_report(9000, 2, 3, 2, 2);
        let w = FitnessWeights::default();
        let e1 = ArenaEntry::from_report("test.ag", &r1, &w);
        let e2 = ArenaEntry::from_report("test.ag", &r2, &w);
        let avg = ArenaEntry::average(&[e1.clone(), e2.clone()]);
        assert_eq!(avg.rounds, 2);
        assert!(avg.error.is_none());
        let expected_score = (e1.score + e2.score) / 2.0;
        assert!((avg.score - expected_score).abs() < 0.001);
    }

    #[test]
    fn entry_average_with_errors() {
        let r1 = make_report(9500, 3, 3, 1, 2);
        let w = FitnessWeights::default();
        let e1 = ArenaEntry::from_report("test.ag", &r1, &w);
        let e2 = ArenaEntry::from_error("test.ag", "CognitiveOverload");
        let avg = ArenaEntry::average(&[e1.clone(), e2]);
        assert_eq!(avg.rounds, 2);
        assert!(avg.error.is_some()); // partial failure
        // Score is average of successful runs only
        assert!((avg.score - e1.score).abs() < 0.001);
    }

    #[test]
    fn entry_average_all_errors() {
        let e1 = ArenaEntry::from_error("test.ag", "error1");
        let e2 = ArenaEntry::from_error("test.ag", "error2");
        let avg = ArenaEntry::average(&[e1, e2]);
        assert_eq!(avg.score, 0.0);
        assert!(avg.error.is_some());
    }

    #[test]
    fn truncate_error_short() {
        assert_eq!(truncate_error("short", 80), "short");
    }

    #[test]
    fn truncate_error_long() {
        let long = "a".repeat(100);
        let t = truncate_error(&long, 80);
        assert_eq!(t.len(), 80);
        assert!(t.ends_with("..."));
    }

    #[test]
    fn format_table_basic() {
        let entries = vec![
            ArenaEntry {
                file: "a.ag".to_string(),
                score: 0.915,
                cb_eff: 0.98,
                val_rate: 1.0,
                exp_rate: 0.67,
                prompt_count: 3,
                error: None,
                rounds: 1,
                worker: None,
                eval_time_ms: None,
            },
            ArenaEntry {
                file: "b.ag".to_string(),
                score: 0.0,
                cb_eff: 0.0,
                val_rate: 0.0,
                exp_rate: 0.0,
                prompt_count: 0,
                error: Some("CognitiveOverload".to_string()),
                rounds: 1,
                worker: None,
                eval_time_ms: None,
            },
        ];
        let table = format_table(&entries, 1);
        assert!(table.contains("Arena: 2 variants, 1 round each"));
        assert!(table.contains("RANK"));
        assert!(table.contains("a.ag"));
        assert!(table.contains("0.915"));
        assert!(table.contains("Winner: a.ag"));
        assert!(table.contains("CognitiveOverload"));
    }

    #[test]
    fn format_json_basic() {
        let entries = vec![ArenaEntry {
            file: "a.ag".to_string(),
            score: 0.9,
            cb_eff: 0.95,
            val_rate: 1.0,
            exp_rate: 0.5,
            prompt_count: 3,
            error: None,
            rounds: 1,
            worker: None,
            eval_time_ms: None,
        }];
        let j = format_json(&entries, 1);
        assert!(j.starts_with("[{"));
        assert!(j.ends_with("}]"));
        assert!(j.contains("\"rank\":1"));
        assert!(j.contains("\"file\":\"a.ag\""));
        assert!(j.contains("\"error\":null"));
        // Single round — no rounds/rounds_avg fields
        assert!(!j.contains("\"rounds\""));
    }

    #[test]
    fn format_json_multi_round() {
        let entries = vec![ArenaEntry {
            file: "a.ag".to_string(),
            score: 0.9,
            cb_eff: 0.95,
            val_rate: 1.0,
            exp_rate: 0.5,
            prompt_count: 3,
            error: None,
            rounds: 3,
            worker: None,
            eval_time_ms: None,
        }];
        let j = format_json(&entries, 3);
        assert!(j.contains("\"rounds\":3"));
        assert!(j.contains("\"rounds_avg\":true"));
    }

    #[test]
    fn format_table_multiple_rounds() {
        let entries = vec![ArenaEntry {
            file: "a.ag".to_string(),
            score: 0.85,
            cb_eff: 0.9,
            val_rate: 1.0,
            exp_rate: 0.5,
            prompt_count: 3,
            error: None,
            rounds: 5,
            worker: None,
            eval_time_ms: None,
        }];
        let table = format_table(&entries, 5);
        assert!(table.contains("5 rounds each"));
    }
}
