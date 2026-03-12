// Fitness scoring for agent evolution (Phase 7).
//
// Composite fitness score F ∈ [0.0, 1.0] from CB efficiency, validate rate,
// and explore rate. Weights are configurable and dynamically redistributed
// when validates or explores are absent.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::json;

// --- Weights ---

#[derive(Debug, Clone)]
pub struct FitnessWeights {
    pub w_cb: f64,
    pub w_val: f64,
    pub w_exp: f64,
}

impl FitnessWeights {
    pub fn new(w_cb: f64, w_val: f64, w_exp: f64) -> Self {
        Self { w_cb, w_val, w_exp }
    }

    /// Parse from comma-separated string: "0.3,0.5,0.2"
    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 3 {
            return Err(format!(
                "expected 3 comma-separated weights, got {}",
                parts.len()
            ));
        }
        let w_cb: f64 = parts[0]
            .trim()
            .parse()
            .map_err(|_| format!("invalid cb weight: '{}'", parts[0].trim()))?;
        let w_val: f64 = parts[1]
            .trim()
            .parse()
            .map_err(|_| format!("invalid val weight: '{}'", parts[1].trim()))?;
        let w_exp: f64 = parts[2]
            .trim()
            .parse()
            .map_err(|_| format!("invalid exp weight: '{}'", parts[2].trim()))?;
        let sum = w_cb + w_val + w_exp;
        if (sum - 1.0).abs() > 0.001 {
            return Err(format!("weights must sum to 1.0, got {sum:.3}"));
        }
        if w_cb < 0.0 || w_val < 0.0 || w_exp < 0.0 {
            return Err("weights must be non-negative".to_string());
        }
        Ok(Self { w_cb, w_val, w_exp })
    }

    pub fn to_string(&self) -> String {
        format!("{},{},{}", self.w_cb, self.w_val, self.w_exp)
    }
}

impl Default for FitnessWeights {
    fn default() -> Self {
        Self {
            w_cb: 0.3,
            w_val: 0.5,
            w_exp: 0.2,
        }
    }
}

// --- Report ---

#[derive(Debug, Clone)]
pub struct FitnessReport {
    pub cb_initial: u64,
    pub cb_remaining: u64,
    pub validates_passed: usize,
    pub validates_total: usize,
    pub explores_passed: usize,
    pub explores_total: usize,
    pub prompt_count: usize,
    pub error: bool,
}

impl FitnessReport {
    /// Compute fitness with default weights.
    pub fn score(&self) -> f64 {
        self.score_with(&FitnessWeights::default())
    }

    /// Compute fitness with given weights and dynamic redistribution.
    pub fn score_with(&self, w: &FitnessWeights) -> f64 {
        if self.error {
            return 0.0;
        }

        let cb_eff = self.cb_efficiency();
        let val_rate = self.validate_rate();
        let exp_rate = self.explore_rate();

        let has_val = self.validates_total > 0;
        let has_exp = self.explores_total > 0;

        match (has_val, has_exp) {
            (true, true) => {
                // All three components active
                w.w_cb * cb_eff + w.w_val * val_rate + w.w_exp * exp_rate
            }
            (true, false) => {
                // No explores: redistribute w_exp proportionally to cb + val
                let total = w.w_cb + w.w_val;
                if total == 0.0 {
                    return cb_eff;
                }
                (w.w_cb / total) * cb_eff + (w.w_val / total) * val_rate
            }
            (false, true) => {
                // No validates: redistribute w_val proportionally to cb + exp
                let total = w.w_cb + w.w_exp;
                if total == 0.0 {
                    return cb_eff;
                }
                (w.w_cb / total) * cb_eff + (w.w_exp / total) * exp_rate
            }
            (false, false) => {
                // No validates AND no explores: F = CB efficiency
                cb_eff
            }
        }
    }

    pub fn cb_efficiency(&self) -> f64 {
        if self.cb_initial == 0 {
            return 1.0;
        }
        self.cb_remaining as f64 / self.cb_initial as f64
    }

    pub fn validate_rate(&self) -> f64 {
        if self.validates_total == 0 {
            return 1.0;
        }
        self.validates_passed as f64 / self.validates_total as f64
    }

    pub fn explore_rate(&self) -> f64 {
        if self.explores_total == 0 {
            return 1.0;
        }
        self.explores_passed as f64 / self.explores_total as f64
    }

    /// Whether this program had no validate/explore blocks.
    pub fn is_cb_only(&self) -> bool {
        self.validates_total == 0 && self.explores_total == 0
    }

    /// Format as human-readable report.
    pub fn display(&self, weights: &FitnessWeights) -> String {
        let mut out = String::new();
        out.push_str("Fitness Report:\n");
        out.push_str(&format!(
            "  CB efficiency:   {:.2} ({}/{})\n",
            self.cb_efficiency(),
            self.cb_remaining,
            self.cb_initial
        ));
        if self.validates_total > 0 {
            out.push_str(&format!(
                "  Validate rate:   {:.2} ({}/{} passed)\n",
                self.validate_rate(),
                self.validates_passed,
                self.validates_total
            ));
        } else {
            out.push_str("  Validate rate:   — (no validates)\n");
        }
        if self.explores_total > 0 {
            out.push_str(&format!(
                "  Explore rate:    {:.2} ({}/{} passed)\n",
                self.explore_rate(),
                self.explores_passed,
                self.explores_total
            ));
        } else {
            out.push_str("  Explore rate:    — (no explores)\n");
        }
        out.push_str(&format!("  Prompt calls:    {}\n", self.prompt_count));
        out.push_str(&format!(
            "  Fitness score:   {:.3}\n",
            self.score_with(weights)
        ));
        if self.is_cb_only() {
            out.push_str("  Warning: No validate/explore blocks — fitness = CB efficiency only.\n");
        }
        out
    }

    /// Serialize as a JSONL entry string (no trailing newline).
    pub fn to_jsonl(&self, source_hash: &str, weights: &FitnessWeights) -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let obj = json::object(vec![
            ("ts", json::JsonValue::Int(ts as i64)),
            (
                "source_hash",
                json::JsonValue::String(source_hash.to_string()),
            ),
            ("score", json::JsonValue::Float(self.score_with(weights))),
            ("cb_eff", json::JsonValue::Float(self.cb_efficiency())),
            ("val_rate", json::JsonValue::Float(self.validate_rate())),
            ("exp_rate", json::JsonValue::Float(self.explore_rate())),
            (
                "prompt_count",
                json::JsonValue::Int(self.prompt_count as i64),
            ),
            ("weights", json::JsonValue::String(weights.to_string())),
        ]);
        format!("{obj}")
    }
}

// --- Registry ---

/// Append a fitness entry to `.agentis/fitness.jsonl`.
pub fn append_to_registry(agentis_root: &Path, entry: &str) -> std::io::Result<()> {
    let path = agentis_root.join("fitness.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{entry}")?;
    Ok(())
}

/// Return the path to the fitness registry.
pub fn registry_path(agentis_root: &Path) -> PathBuf {
    agentis_root.join("fitness.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
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
            prompt_count: 0,
            error: false,
        }
    }

    #[test]
    fn default_weights_full_score() {
        let r = report(10000, 3, 3, 2, 2);
        let score = r.score();
        // 0.3 * 1.0 + 0.5 * 1.0 + 0.2 * 1.0 = 1.0
        assert!((score - 1.0).abs() < 0.001);
    }

    #[test]
    fn default_weights_partial() {
        let r = report(9500, 3, 3, 1, 2);
        let score = r.score();
        // 0.3 * 0.95 + 0.5 * 1.0 + 0.2 * 0.5 = 0.285 + 0.5 + 0.1 = 0.885
        assert!((score - 0.885).abs() < 0.001);
    }

    #[test]
    fn error_gives_zero() {
        let mut r = report(9500, 3, 3, 2, 2);
        r.error = true;
        assert_eq!(r.score(), 0.0);
    }

    #[test]
    fn no_explores_redistributes() {
        let r = report(9500, 3, 3, 0, 0);
        let score = r.score();
        // No explores: redistribute w_exp=0.2 proportionally to w_cb=0.3, w_val=0.5
        // effective: w_cb = 0.3/0.8 = 0.375, w_val = 0.5/0.8 = 0.625
        // 0.375 * 0.95 + 0.625 * 1.0 = 0.35625 + 0.625 = 0.98125
        assert!((score - 0.98125).abs() < 0.001);
    }

    #[test]
    fn no_validates_redistributes() {
        let r = report(9500, 0, 0, 1, 2);
        let score = r.score();
        // No validates: redistribute w_val=0.5 proportionally to w_cb=0.3, w_exp=0.2
        // effective: w_cb = 0.3/0.5 = 0.6, w_exp = 0.2/0.5 = 0.4
        // 0.6 * 0.95 + 0.4 * 0.5 = 0.57 + 0.2 = 0.77
        assert!((score - 0.77).abs() < 0.001);
    }

    #[test]
    fn no_validates_no_explores_cb_only() {
        let r = report(8000, 0, 0, 0, 0);
        let score = r.score();
        // F = CB_efficiency = 0.8
        assert!((score - 0.8).abs() < 0.001);
        assert!(r.is_cb_only());
    }

    #[test]
    fn custom_weights() {
        let r = report(10000, 2, 4, 1, 1);
        let w = FitnessWeights::new(0.4, 0.4, 0.2);
        let score = r.score_with(&w);
        // 0.4 * 1.0 + 0.4 * 0.5 + 0.2 * 1.0 = 0.4 + 0.2 + 0.2 = 0.8
        assert!((score - 0.8).abs() < 0.001);
    }

    #[test]
    fn parse_weights_valid() {
        let w = FitnessWeights::parse("0.4,0.4,0.2").unwrap();
        assert!((w.w_cb - 0.4).abs() < 0.001);
        assert!((w.w_val - 0.4).abs() < 0.001);
        assert!((w.w_exp - 0.2).abs() < 0.001);
    }

    #[test]
    fn parse_weights_bad_sum() {
        let err = FitnessWeights::parse("0.5,0.5,0.5").unwrap_err();
        assert!(err.contains("sum to 1.0"));
    }

    #[test]
    fn parse_weights_wrong_count() {
        let err = FitnessWeights::parse("0.5,0.5").unwrap_err();
        assert!(err.contains("3 comma-separated"));
    }

    #[test]
    fn parse_weights_negative() {
        let err = FitnessWeights::parse("-0.1,0.6,0.5").unwrap_err();
        assert!(err.contains("non-negative"));
    }

    #[test]
    fn jsonl_format() {
        let r = report(9500, 3, 3, 1, 2);
        let line = r.to_jsonl("abc123", &FitnessWeights::default());
        assert!(line.contains("\"source_hash\":\"abc123\""));
        assert!(line.contains("\"prompt_count\":0"));
        assert!(line.contains("\"weights\":\"0.3,0.5,0.2\""));
    }

    #[test]
    fn display_report_cb_only_warning() {
        let r = report(8000, 0, 0, 0, 0);
        let text = r.display(&FitnessWeights::default());
        assert!(text.contains("CB efficiency only"));
        assert!(text.contains("no validates"));
        assert!(text.contains("no explores"));
    }

    #[test]
    fn display_report_normal() {
        let r = report(9500, 3, 3, 1, 2);
        let text = r.display(&FitnessWeights::default());
        assert!(text.contains("3/3 passed"));
        assert!(text.contains("1/2 passed"));
        assert!(!text.contains("CB efficiency only"));
    }

    #[test]
    fn cb_efficiency_zero_initial() {
        let mut r = report(0, 0, 0, 0, 0);
        r.cb_initial = 0;
        assert_eq!(r.cb_efficiency(), 1.0);
    }

    #[test]
    fn append_registry() {
        let dir = std::env::temp_dir().join(format!("agentis_fitness_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        append_to_registry(&dir, r#"{"test":true}"#).unwrap();
        append_to_registry(&dir, r#"{"test":false}"#).unwrap();
        let content = std::fs::read_to_string(dir.join("fitness.jsonl")).unwrap();
        assert_eq!(content.lines().count(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
