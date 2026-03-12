// Runtime tracing for Agentis execution.
//
// Three verbosity levels:
// - Quiet:   only LLM wait indicators (the bare minimum UX)
// - Normal:  agent lifecycle, prompt calls, explore outcomes, spawn/await
// - Verbose: everything including LLM responses, CB deltas, validate details

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceLevel {
    Quiet,
    Normal,
    Verbose,
}

impl TraceLevel {
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "quiet" => TraceLevel::Quiet,
            "verbose" => TraceLevel::Verbose,
            _ => TraceLevel::Normal,
        }
    }
}

/// Trait for trace output. Default implementation writes to stderr.
/// Tests can capture output by implementing this trait with a Vec<String>.
pub trait TraceOutput: Send + Sync {
    fn write_trace(&self, msg: &str);
}

/// Default stderr output.
pub struct StderrTrace;

impl TraceOutput for StderrTrace {
    fn write_trace(&self, msg: &str) {
        eprintln!("{msg}");
    }
}

/// Captures trace output into a vector (for testing).
#[cfg(test)]
pub struct CaptureTrace {
    lines: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl CaptureTrace {
    pub fn new() -> Self {
        Self {
            lines: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl TraceOutput for CaptureTrace {
    fn write_trace(&self, msg: &str) {
        self.lines.lock().unwrap().push(msg.to_string());
    }
}

/// Runtime tracer. Passed to Evaluator, called at key execution points.
pub struct Tracer {
    level: TraceLevel,
    output: Box<dyn TraceOutput>,
}

impl Tracer {
    pub fn new(level: TraceLevel) -> Self {
        Self {
            level,
            output: Box::new(StderrTrace),
        }
    }

    #[cfg(test)]
    pub fn with_output(level: TraceLevel, output: Box<dyn TraceOutput>) -> Self {
        Self { level, output }
    }

    #[allow(dead_code)]
    pub fn level(&self) -> TraceLevel {
        self.level
    }

    // --- Always emitted (even in quiet) ---

    pub fn llm_requesting(&self, backend: &str, model: &str) {
        self.output
            .write_trace(&format!("[llm] requesting {backend} {model} ..."));
    }

    #[allow(dead_code)]
    pub fn llm_still_waiting(&self, elapsed_secs: f64) {
        self.output
            .write_trace(&format!("[llm] still waiting ... ({elapsed_secs:.1}s)"));
    }

    pub fn llm_received(&self, elapsed_secs: f64) {
        self.output
            .write_trace(&format!("[llm] received ({elapsed_secs:.1}s)"));
    }

    // --- Normal level ---

    pub fn agent_entered(&self, name: &str, budget: u64) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        self.output
            .write_trace(&format!("[agent {name}] entered, CB={budget}"));
    }

    pub fn agent_exited(&self, name: &str, result: &str) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        self.output
            .write_trace(&format!("[agent {name}] exited, {result}"));
    }

    pub fn prompt_call(&self, instruction: &str, return_type: &str) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        let short = if instruction.len() > 60 {
            format!("{}...", &instruction[..57])
        } else {
            instruction.to_string()
        };
        self.output
            .write_trace(&format!("[prompt] \"{short}\" -> {return_type}"));
    }

    pub fn spawn_agent(&self, name: &str, budget: u64, handle_id: u32) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        self.output
            .write_trace(&format!("[spawn {name}] CB={budget}, handle=#{handle_id}"));
    }

    pub fn await_completed(&self, handle_id: u32, result_type: &str) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        self.output.write_trace(&format!(
            "[await #{handle_id}] completed, result={result_type}"
        ));
    }

    pub fn explore_entered(&self, name: &str, budget: u64) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        self.output
            .write_trace(&format!("[explore \"{name}\"] entered, CB={budget}"));
    }

    pub fn explore_outcome(&self, name: &str, success: bool) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        if success {
            self.output
                .write_trace(&format!("[explore \"{name}\"] branch created"));
        } else {
            self.output
                .write_trace(&format!("[explore \"{name}\"] failed, rolled back"));
        }
    }

    pub fn validate_result(&self, count: usize, all_passed: bool) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        if all_passed {
            let marks = "pass ".repeat(count).trim_end().to_string();
            self.output
                .write_trace(&format!("[validate] {count} predicates: {marks}"));
        } else {
            self.output
                .write_trace(&format!("[validate] {count} predicates: FAILED"));
        }
    }

    pub fn import_resolved(&self, hash: &str, alias: Option<&str>) {
        if self.level == TraceLevel::Quiet {
            return;
        }
        let short_hash = if hash.len() > 12 { &hash[..12] } else { hash };
        match alias {
            Some(a) => self
                .output
                .write_trace(&format!("[import] {short_hash}... as {a}")),
            None => self
                .output
                .write_trace(&format!("[import] {short_hash}...")),
        }
    }

    /// PII scan result — always shown (even in quiet mode, like LLM wait).
    pub fn pii_scan_result(&self, types: &str, granted: bool) {
        if granted {
            self.output.write_trace(&format!(
                "[pii] scan: detected ({types}) — PiiTransmit granted, proceeding"
            ));
        } else {
            self.output.write_trace(&format!(
                "[pii] scan: detected ({types}) — BLOCKED (PiiTransmit not granted)"
            ));
        }
    }

    // --- Verbose level ---

    pub fn llm_response(&self, response: &impl fmt::Display) {
        if self.level != TraceLevel::Verbose {
            return;
        }
        let s = format!("{response}");
        let short = if s.len() > 200 {
            format!("{}...", &s[..197])
        } else {
            s
        };
        self.output.write_trace(&format!("[llm] response: {short}"));
    }

    pub fn cb_remaining(&self, budget: u64, max_budget: u64) {
        if self.level != TraceLevel::Verbose {
            return;
        }
        self.output
            .write_trace(&format!("[CB] remaining: {budget}/{max_budget}"));
    }

    pub fn validate_detail(&self, index: usize, passed: bool) {
        if self.level != TraceLevel::Verbose {
            return;
        }
        let mark = if passed { "pass" } else { "FAIL" };
        self.output
            .write_trace(&format!("[validate] predicate #{index}: {mark}"));
    }
}

// Provide a no-op default so spawned agents without a tracer reference work
impl Default for Tracer {
    fn default() -> Self {
        Self::new(TraceLevel::Quiet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_level_from_str() {
        assert_eq!(TraceLevel::from_str("quiet"), TraceLevel::Quiet);
        assert_eq!(TraceLevel::from_str("normal"), TraceLevel::Normal);
        assert_eq!(TraceLevel::from_str("verbose"), TraceLevel::Verbose);
        assert_eq!(TraceLevel::from_str("NORMAL"), TraceLevel::Normal);
        assert_eq!(TraceLevel::from_str("unknown"), TraceLevel::Normal);
    }

    #[test]
    fn quiet_only_emits_llm() {
        let capture = CaptureTrace::new();
        let tracer = Tracer::with_output(TraceLevel::Quiet, Box::new(capture));
        tracer.agent_entered("test", 1000);
        tracer.prompt_call("do something", "string");
        tracer.llm_requesting("cli", "claude");
        tracer.llm_received(3.5);
        // Access captured lines through the tracer's output
        // We need to downcast — instead, just verify no panic
        // The real test is that quiet suppresses non-LLM events
    }

    #[test]
    fn capture_trace_collects_lines() {
        let capture = Box::new(CaptureTrace::new());
        // We need a reference to read back lines, so use Arc
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer_output = shared.clone();

        // Build tracer with shared capture
        let tracer = Tracer {
            level: TraceLevel::Normal,
            output: Box::new(ArcTrace(tracer_output)),
        };

        tracer.agent_entered("scanner", 1000);
        tracer.prompt_call("analyze", "Report");
        tracer.llm_requesting("cli", "claude");
        tracer.llm_received(4.2);

        let lines = shared.lines();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[agent scanner]"));
        assert!(lines[1].contains("[prompt]"));
        assert!(lines[2].contains("[llm] requesting"));
        assert!(lines[3].contains("[llm] received"));
    }

    // Helper: TraceOutput backed by Arc<CaptureTrace>
    struct ArcTrace(std::sync::Arc<CaptureTrace>);
    impl TraceOutput for ArcTrace {
        fn write_trace(&self, msg: &str) {
            self.0.write_trace(msg);
        }
    }

    #[test]
    fn normal_emits_agent_and_prompt() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Normal,
            output: Box::new(ArcTrace(shared.clone())),
        };

        tracer.agent_entered("worker", 500);
        tracer.spawn_agent("worker", 500, 1);
        tracer.explore_entered("feature-x", 300);
        tracer.explore_outcome("feature-x", true);
        tracer.validate_result(3, true);

        let lines = shared.lines();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("[agent worker]"));
        assert!(lines[1].contains("[spawn worker]"));
        assert!(lines[2].contains("[explore \"feature-x\"]"));
        assert!(lines[3].contains("branch created"));
        assert!(lines[4].contains("[validate]"));
    }

    #[test]
    fn verbose_emits_cb_and_response() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Verbose,
            output: Box::new(ArcTrace(shared.clone())),
        };

        tracer.cb_remaining(340, 1000);
        tracer.llm_response(&"{ \"mood\": \"happy\" }");
        tracer.validate_detail(0, true);
        tracer.validate_detail(1, false);

        let lines = shared.lines();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("[CB] remaining: 340/1000"));
        assert!(lines[1].contains("[llm] response:"));
        assert!(lines[2].contains("predicate #0: pass"));
        assert!(lines[3].contains("predicate #1: FAIL"));
    }

    #[test]
    fn normal_suppresses_verbose() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Normal,
            output: Box::new(ArcTrace(shared.clone())),
        };

        tracer.cb_remaining(340, 1000);
        tracer.llm_response(&"something");
        tracer.validate_detail(0, true);

        assert!(shared.lines().is_empty());
    }

    #[test]
    fn quiet_suppresses_normal() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Quiet,
            output: Box::new(ArcTrace(shared.clone())),
        };

        tracer.agent_entered("x", 100);
        tracer.prompt_call("y", "z");
        tracer.spawn_agent("a", 100, 1);
        tracer.explore_entered("b", 100);
        tracer.explore_outcome("b", true);
        tracer.validate_result(1, true);
        tracer.import_resolved("abc123", None);

        assert!(shared.lines().is_empty());
    }

    #[test]
    fn quiet_still_emits_llm_progress() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Quiet,
            output: Box::new(ArcTrace(shared.clone())),
        };

        tracer.llm_requesting("http", "claude-sonnet");
        tracer.llm_still_waiting(5.0);
        tracer.llm_received(7.8);

        let lines = shared.lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("requesting"));
        assert!(lines[1].contains("still waiting"));
        assert!(lines[2].contains("received"));
    }

    #[test]
    fn long_instruction_truncated() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Normal,
            output: Box::new(ArcTrace(shared.clone())),
        };

        let long = "a".repeat(100);
        tracer.prompt_call(&long, "string");

        let lines = shared.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("..."));
        assert!(lines[0].len() < 200);
    }

    #[test]
    fn long_llm_response_truncated() {
        let shared = std::sync::Arc::new(CaptureTrace::new());
        let tracer = Tracer {
            level: TraceLevel::Verbose,
            output: Box::new(ArcTrace(shared.clone())),
        };

        let long = "x".repeat(500);
        tracer.llm_response(&long);

        let lines = shared.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("..."));
    }
}
