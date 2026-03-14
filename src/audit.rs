// Audit logging for prompt calls.
//
// Writes JSONL entries to `.agentis/audit/prompts.jsonl`.
// Opt-in: enabled when the directory exists (created by `agentis init --secure`
// or manually via `mkdir -p .agentis/audit`).
//
// Each prompt() call logs: timestamp, agent context, instruction/input hashes,
// PII scan result, capability status, backend info.

use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{self, JsonValue};
use crate::pii::PiiScanResult;

/// A single audit entry for a prompt() call.
pub struct PromptAuditEntry<'a> {
    pub agent_name: Option<&'a str>,
    pub instruction: &'a str,
    pub input: &'a str,
    pub pii_result: &'a PiiScanResult,
    pub pii_transmit_granted: bool,
    pub backend_name: &'a str,
    pub model: &'a str,
    pub cb_cost: u64,
}

/// JSONL audit logger. Thread-safe via Mutex around file handle.
pub struct AuditLog {
    file: Mutex<File>,
}

impl AuditLog {
    /// Open or create the audit log. Returns None if the audit directory
    /// doesn't exist (audit is opt-in).
    pub fn open(agentis_root: &Path) -> Option<Self> {
        let audit_dir = agentis_root.join("audit");
        if !audit_dir.is_dir() {
            return None;
        }
        let path = audit_dir.join("prompts.jsonl");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        Some(Self {
            file: Mutex::new(file),
        })
    }

    /// Log a prompt() call as a JSONL line.
    pub fn log_prompt(&self, entry: &PromptAuditEntry<'_>) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let instruction_hash = sha256_short(entry.instruction);
        let input_hash = sha256_short(entry.input);
        let input_len = entry.input.len() as i64;

        let (pii_scan, pii_types) = if entry.pii_result.is_clean() {
            ("clean".to_string(), json::array(vec![]))
        } else {
            (
                "detected".to_string(),
                json::array(
                    entry
                        .pii_result
                        .detected
                        .iter()
                        .map(|t| JsonValue::String(t.to_string()))
                        .collect(),
                ),
            )
        };

        let agent_val = match entry.agent_name {
            Some(name) => JsonValue::String(name.to_string()),
            None => JsonValue::String("(top-level)".to_string()),
        };

        let obj = json::object(vec![
            ("ts", JsonValue::Int(ts)),
            ("agent", agent_val),
            ("instruction_hash", JsonValue::String(instruction_hash)),
            ("input_hash", JsonValue::String(input_hash)),
            ("input_len", JsonValue::Int(input_len)),
            ("pii_scan", JsonValue::String(pii_scan)),
            ("pii_types", pii_types),
            (
                "pii_transmit_granted",
                JsonValue::Bool(entry.pii_transmit_granted),
            ),
            ("backend", JsonValue::String(entry.backend_name.to_string())),
            ("model", JsonValue::String(entry.model.to_string())),
            ("cb_cost", JsonValue::Int(entry.cb_cost as i64)),
        ]);

        let line = format!("{obj}\n");
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Log a confidence() call as a JSONL line.
    pub fn log_confidence(
        &self,
        instruction: &str,
        sample_count: usize,
        agreement: f64,
        confidence: f64,
    ) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let instruction_hash = sha256_short(instruction);
        let obj = json::object(vec![
            ("ts", JsonValue::Int(ts)),
            ("type", JsonValue::String("confidence".to_string())),
            ("instruction_hash", JsonValue::String(instruction_hash)),
            ("sample_count", JsonValue::Int(sample_count as i64)),
            ("agreement", JsonValue::Float(agreement)),
            ("confidence", JsonValue::Float(confidence)),
        ]);
        let line = format!("{obj}\n");
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

pub(crate) fn sha256_short(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    // First 12 hex chars
    result.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii;
    use std::path::PathBuf;

    fn temp_audit_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let audit_dir = root.join("audit");
        std::fs::create_dir_all(&audit_dir).unwrap();
        (dir, root)
    }

    #[test]
    fn open_returns_none_without_audit_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AuditLog::open(dir.path()).is_none());
    }

    #[test]
    fn open_returns_some_with_audit_dir() {
        let (_dir, root) = temp_audit_dir();
        assert!(AuditLog::open(&root).is_some());
    }

    #[test]
    fn log_prompt_writes_jsonl() {
        let (_dir, root) = temp_audit_dir();
        let log = AuditLog::open(&root).unwrap();
        let pii_result = pii::scan("Hello world");
        let entry = PromptAuditEntry {
            agent_name: Some("analyzer"),
            instruction: "Summarize this",
            input: "Hello world",
            pii_result: &pii_result,
            pii_transmit_granted: false,
            backend_name: "mock",
            model: "",
            cb_cost: 50,
        };
        log.log_prompt(&entry);
        drop(log);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);

        let parsed = crate::json::parse(lines[0]).unwrap();
        assert_eq!(parsed.get("agent").unwrap().as_str(), Some("analyzer"));
        assert_eq!(parsed.get("pii_scan").unwrap().as_str(), Some("clean"));
        assert_eq!(
            parsed.get("pii_transmit_granted").unwrap().as_bool(),
            Some(false)
        );
        assert_eq!(parsed.get("backend").unwrap().as_str(), Some("mock"));
        assert!(parsed.get("ts").unwrap().as_i64().is_some());
        assert!(
            parsed
                .get("instruction_hash")
                .unwrap()
                .as_str()
                .unwrap()
                .len()
                == 12
        );
        assert!(parsed.get("input_hash").unwrap().as_str().unwrap().len() == 12);
        assert_eq!(parsed.get("input_len").unwrap().as_i64(), Some(11));
    }

    #[test]
    fn log_prompt_with_pii() {
        let (_dir, root) = temp_audit_dir();
        let log = AuditLog::open(&root).unwrap();
        let pii_result = pii::scan("Contact user@test.com or call +420 123 456 789");
        let entry = PromptAuditEntry {
            agent_name: Some("scanner"),
            instruction: "Analyze contacts",
            input: "Contact user@test.com or call +420 123 456 789",
            pii_result: &pii_result,
            pii_transmit_granted: true,
            backend_name: "cli",
            model: "claude",
            cb_cost: 50,
        };
        log.log_prompt(&entry);
        drop(log);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let parsed = crate::json::parse(content.trim()).unwrap();
        assert_eq!(parsed.get("pii_scan").unwrap().as_str(), Some("detected"));
        assert_eq!(
            parsed.get("pii_transmit_granted").unwrap().as_bool(),
            Some(true)
        );
        assert_eq!(parsed.get("backend").unwrap().as_str(), Some("cli"));
        assert_eq!(parsed.get("model").unwrap().as_str(), Some("claude"));

        let pii_types = parsed.get("pii_types").unwrap().as_array().unwrap();
        assert!(pii_types.len() >= 2);
    }

    #[test]
    fn log_prompt_no_agent_name() {
        let (_dir, root) = temp_audit_dir();
        let log = AuditLog::open(&root).unwrap();
        let pii_result = pii::scan("clean text");
        let entry = PromptAuditEntry {
            agent_name: None,
            instruction: "Do something",
            input: "clean text",
            pii_result: &pii_result,
            pii_transmit_granted: false,
            backend_name: "mock",
            model: "",
            cb_cost: 50,
        };
        log.log_prompt(&entry);
        drop(log);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let parsed = crate::json::parse(content.trim()).unwrap();
        assert_eq!(parsed.get("agent").unwrap().as_str(), Some("(top-level)"));
    }

    #[test]
    fn multiple_entries_append() {
        let (_dir, root) = temp_audit_dir();
        let log = AuditLog::open(&root).unwrap();
        let pii_result = pii::scan("ok");

        for i in 0..3 {
            let instr = format!("instruction {i}");
            let entry = PromptAuditEntry {
                agent_name: Some("worker"),
                instruction: &instr,
                input: "data",
                pii_result: &pii_result,
                pii_transmit_granted: false,
                backend_name: "mock",
                model: "",
                cb_cost: 50,
            };
            log.log_prompt(&entry);
        }
        drop(log);

        let content = std::fs::read_to_string(root.join("audit/prompts.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);

        // Each line is valid JSON
        for line in &lines {
            assert!(crate::json::parse(line).is_ok());
        }
    }

    #[test]
    fn sha256_short_deterministic() {
        let h1 = sha256_short("hello");
        let h2 = sha256_short("hello");
        let h3 = sha256_short("world");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 12);
    }

    #[test]
    fn sha256_short_hex_chars() {
        let h = sha256_short("test input");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
