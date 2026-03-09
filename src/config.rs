// Configuration reader for Agentis.
//
// Format: simple `key = value` lines, `#` comments, no nesting.
// File: `.agentis/config`

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    values: HashMap<String, String>,
}

impl Config {
    /// Load config from `.agentis/config`. Returns empty config if file missing.
    pub fn load(agentis_root: &Path) -> Self {
        let path = agentis_root.join("config");
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        Self::parse(&content)
    }

    /// Parse config from string content.
    pub fn parse(content: &str) -> Self {
        let mut values = HashMap::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = trimmed.split_once('=') {
                values.insert(key.trim().to_string(), val.trim().to_string());
            }
        }
        Self { values }
    }

    /// Get a config value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(|s| s.as_str())
    }

    /// Get a config value with a default fallback.
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.values
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    /// Get a config value as u64.
    pub fn get_u64(&self, key: &str, default: u64) -> u64 {
        self.values
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let cfg = Config::parse("");
        assert_eq!(cfg.get("anything"), None);
    }

    #[test]
    fn parse_simple() {
        let cfg = Config::parse("llm.backend = http\nllm.model = claude-sonnet-4-20250514");
        assert_eq!(cfg.get("llm.backend"), Some("http"));
        assert_eq!(cfg.get("llm.model"), Some("claude-sonnet-4-20250514"));
    }

    #[test]
    fn parse_comments_and_blanks() {
        let cfg = Config::parse("# comment\n\nkey = value\n  # another");
        assert_eq!(cfg.get("key"), Some("value"));
    }

    #[test]
    fn parse_whitespace_trimming() {
        let cfg = Config::parse("  key  =  value with spaces  ");
        assert_eq!(cfg.get("key"), Some("value with spaces"));
    }

    #[test]
    fn get_or_default() {
        let cfg = Config::parse("a = 1");
        assert_eq!(cfg.get_or("a", "x"), "1");
        assert_eq!(cfg.get_or("b", "x"), "x");
    }

    #[test]
    fn get_u64_default() {
        let cfg = Config::parse("retries = 3\nbad = abc");
        assert_eq!(cfg.get_u64("retries", 0), 3);
        assert_eq!(cfg.get_u64("bad", 5), 5);
        assert_eq!(cfg.get_u64("missing", 10), 10);
    }

    #[test]
    fn load_missing_file() {
        let cfg = Config::load(Path::new("/nonexistent/path"));
        assert_eq!(cfg.get("anything"), None);
    }
}
