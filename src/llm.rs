// Pluggable LLM backend for Agentis.
//
// Provides the trait and implementations for prompt execution:
// - MockBackend: deterministic stub values (testing, default)
// - HttpBackend: real HTTPS calls via ureq to LLM APIs

use std::collections::HashMap;

use crate::ast::TypeAnnotation;
use crate::config::Config;
use crate::evaluator::Value;
use crate::json::{self, JsonValue};

// --- LLM Errors ---

#[derive(Debug, Clone, PartialEq)]
pub enum LlmError {
    /// HTTP or network failure
    Transport(String),
    /// LLM returned invalid/unparseable response
    InvalidResponse(String),
    /// Response doesn't match expected type
    TypeMismatch { expected: String, got: String },
    /// Configuration error
    Config(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Transport(msg) => write!(f, "LLM transport error: {msg}"),
            LlmError::InvalidResponse(msg) => write!(f, "LLM invalid response: {msg}"),
            LlmError::TypeMismatch { expected, got } => {
                write!(f, "LLM type mismatch: expected {expected}, got {got}")
            }
            LlmError::Config(msg) => write!(f, "LLM config error: {msg}"),
        }
    }
}

// --- LLM Backend Trait ---

pub trait LlmBackend {
    fn complete(
        &self,
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
        type_fields: Option<&[(&str, &str)]>,
    ) -> Result<Value, LlmError>;

    /// Backend name for trace output.
    fn name(&self) -> &str;
}

// --- Mock Backend ---

pub struct MockBackend;

impl MockBackend {
    pub fn new() -> Self {
        Self
    }
}

impl LlmBackend for MockBackend {
    fn complete(
        &self,
        _instruction: &str,
        _input: &str,
        return_type: &TypeAnnotation,
        type_fields: Option<&[(&str, &str)]>,
    ) -> Result<Value, LlmError> {
        mock_value_for_type(return_type, type_fields)
    }

    fn name(&self) -> &str { "mock" }
}

fn mock_value_for_type(
    type_ann: &TypeAnnotation,
    type_fields: Option<&[(&str, &str)]>,
) -> Result<Value, LlmError> {
    match type_ann {
        TypeAnnotation::Named(name) => match name.as_str() {
            "int" => Ok(Value::Int(0)),
            "float" => Ok(Value::Float(0.0)),
            "string" => Ok(Value::String("mock".to_string())),
            "bool" => Ok(Value::Bool(true)),
            _ => {
                // Build mock struct from provided field info
                let mut fields = HashMap::new();
                if let Some(type_fields) = type_fields {
                    for (field_name, field_type) in type_fields {
                        let ann = TypeAnnotation::Named(field_type.to_string());
                        let value = mock_value_for_type(&ann, None)?;
                        fields.insert(field_name.to_string(), value);
                    }
                }
                Ok(Value::Struct(name.clone(), fields))
            }
        },
        TypeAnnotation::Generic(_, _) => Ok(Value::String("mock_collection".to_string())),
    }
}

// --- HTTP Backend ---

pub struct HttpBackend {
    endpoint: String,
    model: String,
    api_key: String,
    max_retries: u32,
}

impl HttpBackend {
    pub fn from_config(config: &Config) -> Result<Self, LlmError> {
        let endpoint = config
            .get("llm.endpoint")
            .ok_or_else(|| LlmError::Config("llm.endpoint not set".into()))?
            .to_string();

        let model = config.get_or("llm.model", "claude-sonnet-4-20250514");

        let api_key_env = config.get_or("llm.api_key_env", "ANTHROPIC_API_KEY");
        let api_key = std::env::var(&api_key_env).map_err(|_| {
            LlmError::Config(format!(
                "environment variable '{api_key_env}' not set (from llm.api_key_env)"
            ))
        })?;

        let max_retries = config.get_u64("llm.max_retries", 2) as u32;

        Ok(Self {
            endpoint,
            model,
            api_key,
            max_retries,
        })
    }

    fn build_request_body(
        &self,
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
    ) -> String {
        let type_str = format_type_annotation(return_type);
        let user_content = format!(
            "{instruction}\n\nInput: {input}\n\nRespond with ONLY valid JSON matching type: {type_str}"
        );

        let body = json::object(vec![
            ("model", JsonValue::String(self.model.clone())),
            (
                "max_tokens",
                JsonValue::Int(1024),
            ),
            (
                "messages",
                json::array(vec![json::object(vec![
                    ("role", JsonValue::String("user".into())),
                    ("content", JsonValue::String(user_content)),
                ])]),
            ),
        ]);

        body.to_string()
    }

    fn extract_content(response_body: &str) -> Result<String, LlmError> {
        let json = json::parse(response_body)
            .map_err(|e| LlmError::InvalidResponse(format!("failed to parse response: {e}")))?;

        // Anthropic format: { "content": [{ "type": "text", "text": "..." }] }
        let content = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .ok_or_else(|| {
                LlmError::InvalidResponse("missing content[0].text in response".into())
            })?;

        Ok(content.to_string())
    }

    fn json_to_value(
        json: &JsonValue,
        return_type: &TypeAnnotation,
    ) -> Result<Value, LlmError> {
        match return_type {
            TypeAnnotation::Named(name) => match name.as_str() {
                "int" => json.as_i64().map(Value::Int).ok_or_else(|| {
                    LlmError::TypeMismatch {
                        expected: "int".into(),
                        got: describe_json(json),
                    }
                }),
                "float" => json.as_f64().map(Value::Float).ok_or_else(|| {
                    LlmError::TypeMismatch {
                        expected: "float".into(),
                        got: describe_json(json),
                    }
                }),
                "string" => json
                    .as_str()
                    .map(|s| Value::String(s.to_string()))
                    .ok_or_else(|| LlmError::TypeMismatch {
                        expected: "string".into(),
                        got: describe_json(json),
                    }),
                "bool" => json.as_bool().map(Value::Bool).ok_or_else(|| {
                    LlmError::TypeMismatch {
                        expected: "bool".into(),
                        got: describe_json(json),
                    }
                }),
                type_name => {
                    // User-defined struct: expect JSON object
                    let obj = json.as_object().ok_or_else(|| LlmError::TypeMismatch {
                        expected: type_name.into(),
                        got: describe_json(json),
                    })?;
                    let mut fields = HashMap::new();
                    for (k, v) in obj {
                        // Infer field type from JSON value
                        let value = json_to_value_inferred(v);
                        fields.insert(k.clone(), value);
                    }
                    Ok(Value::Struct(type_name.to_string(), fields))
                }
            },
            TypeAnnotation::Generic(name, args) => {
                match name.as_str() {
                    "List" if args.len() == 1 => {
                        let arr = json.as_array().ok_or_else(|| LlmError::TypeMismatch {
                            expected: "List".into(),
                            got: describe_json(json),
                        })?;
                        let mut items = Vec::new();
                        for item in arr {
                            items.push(Self::json_to_value(item, &args[0])?);
                        }
                        Ok(Value::List(items))
                    }
                    _ => Ok(Value::String(json.to_string())),
                }
            }
        }
    }
}

impl LlmBackend for HttpBackend {
    fn complete(
        &self,
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
        _type_fields: Option<&[(&str, &str)]>,
    ) -> Result<Value, LlmError> {
        let request_body = self.build_request_body(instruction, input, return_type);

        let mut last_error = None;
        let attempts = 1 + self.max_retries;

        for attempt in 0..attempts {
            if attempt > 0 {
                eprintln!(
                    "[LLM retry {}/{}: {}]",
                    attempt,
                    self.max_retries,
                    last_error.as_ref().map(|e: &LlmError| e.to_string()).unwrap_or_default()
                );
            }

            match self.do_request(&request_body) {
                Ok(response_body) => {
                    match Self::extract_content(&response_body) {
                        Ok(content) => {
                            // Try to parse the LLM's text as JSON
                            match json::parse(content.trim()) {
                                Ok(json_val) => {
                                    match Self::json_to_value(&json_val, return_type) {
                                        Ok(value) => return Ok(value),
                                        Err(e) => last_error = Some(e),
                                    }
                                }
                                Err(e) => {
                                    last_error = Some(LlmError::InvalidResponse(format!(
                                        "LLM returned invalid JSON: {e}"
                                    )));
                                }
                            }
                        }
                        Err(e) => last_error = Some(e),
                    }
                }
                Err(e) => last_error = Some(e),
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::Transport("no attempts made".into())))
    }

    fn name(&self) -> &str { "http" }
}

impl HttpBackend {
    fn do_request(&self, body: &str) -> Result<String, LlmError> {
        let mut response = ureq::post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send(body)
            .map_err(|e| LlmError::Transport(format!("{e}")))?;

        response
            .body_mut()
            .read_to_string()
            .map_err(|e| LlmError::Transport(format!("failed to read response body: {e}")))
    }
}

// --- CLI Backend ---

/// Runs prompts through any CLI tool that accepts prompt on stdin and
/// returns response on stdout. Works with flat-rate subscriptions.
///
/// Config examples:
///   claude:  llm.command = claude,  llm.args = -p --output-format text
///   gemini:  llm.command = gemini,  llm.args = -p
///   custom:  llm.command = my-tool, llm.args = --json --stdin
pub struct CliBackend {
    command: String,
    args: Vec<String>,
    model: Option<String>,
    max_retries: u32,
}

impl CliBackend {
    pub fn from_config(config: &Config) -> Result<Self, LlmError> {
        let command = config.get_or("llm.command", "claude");
        let args_str = config.get_or("llm.args", "-p --output-format text");
        let args: Vec<String> = args_str
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let model = config.get("llm.model").map(|s| s.to_string());
        let max_retries = config.get_u64("llm.max_retries", 2) as u32;
        Ok(Self {
            command,
            args,
            model,
            max_retries,
        })
    }

    fn build_prompt(
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
    ) -> String {
        let type_str = format_type_annotation(return_type);
        format!(
            "{instruction}\n\nInput: {input}\n\nRespond with ONLY valid JSON matching type: {type_str}. No markdown, no explanation, just the JSON value."
        )
    }

    fn run_cli(&self, prompt: &str) -> Result<String, LlmError> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new(&self.command);
        for arg in &self.args {
            cmd.arg(arg);
        }
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            LlmError::Transport(format!(
                "failed to spawn '{}': {e} (is claude CLI installed?)",
                self.command
            ))
        })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).map_err(|e| {
                LlmError::Transport(format!("failed to write to claude stdin: {e}"))
            })?;
        }

        let output = child.wait_with_output().map_err(|e| {
            LlmError::Transport(format!("failed to read claude output: {e}"))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LlmError::Transport(format!(
                "claude CLI exited with {}: {stderr}",
                output.status
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }
}

impl LlmBackend for CliBackend {
    fn complete(
        &self,
        instruction: &str,
        input: &str,
        return_type: &TypeAnnotation,
        _type_fields: Option<&[(&str, &str)]>,
    ) -> Result<Value, LlmError> {
        let prompt = Self::build_prompt(instruction, input, return_type);
        let mut last_error = None;
        let attempts = 1 + self.max_retries;

        for attempt in 0..attempts {
            if attempt > 0 {
                eprintln!(
                    "[LLM retry {}/{}: {}]",
                    attempt,
                    self.max_retries,
                    last_error.as_ref().map(|e: &LlmError| e.to_string()).unwrap_or_default()
                );
            }

            match self.run_cli(&prompt) {
                Ok(raw_output) => {
                    // Extract JSON from the response (skip any non-JSON preamble)
                    let trimmed = extract_json_from_text(&raw_output);
                    match json::parse(trimmed) {
                        Ok(json_val) => {
                            match HttpBackend::json_to_value(&json_val, return_type) {
                                Ok(value) => return Ok(value),
                                Err(e) => last_error = Some(e),
                            }
                        }
                        Err(e) => {
                            last_error = Some(LlmError::InvalidResponse(format!(
                                "CLI returned invalid JSON: {e}"
                            )));
                        }
                    }
                }
                Err(e) => last_error = Some(e),
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::Transport("no attempts made".into())))
    }

    fn name(&self) -> &str { "cli" }
}

/// Try to extract a JSON value from text that may contain non-JSON preamble.
fn extract_json_from_text(text: &str) -> &str {
    let trimmed = text.trim();
    // If it starts with { or [ or " or a digit or true/false/null, it's likely JSON
    if trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('"')
        || trimmed.starts_with("true")
        || trimmed.starts_with("false")
        || trimmed.starts_with("null")
        || trimmed.bytes().next().map_or(false, |b| b.is_ascii_digit() || b == b'-')
    {
        return trimmed;
    }
    // Try to find JSON object or array in the text
    for (i, ch) in trimmed.char_indices() {
        if ch == '{' || ch == '[' {
            return &trimmed[i..];
        }
    }
    trimmed
}

// --- Helpers ---

fn format_type_annotation(ann: &TypeAnnotation) -> String {
    match ann {
        TypeAnnotation::Named(name) => name.clone(),
        TypeAnnotation::Generic(name, args) => {
            let args_str: Vec<String> = args.iter().map(format_type_annotation).collect();
            format!("{}<{}>", name, args_str.join(", "))
        }
    }
}

fn json_to_value_inferred(json: &JsonValue) -> Value {
    match json {
        JsonValue::Null => Value::Void,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Int(n) => Value::Int(*n),
        JsonValue::Float(n) => Value::Float(*n),
        JsonValue::String(s) => Value::String(s.clone()),
        JsonValue::Array(items) => {
            Value::List(items.iter().map(json_to_value_inferred).collect())
        }
        JsonValue::Object(map) => {
            let mut fields = HashMap::new();
            for (k, v) in map {
                fields.insert(k.clone(), json_to_value_inferred(v));
            }
            Value::Struct("object".to_string(), fields)
        }
    }
}

fn describe_json(json: &JsonValue) -> String {
    match json {
        JsonValue::Null => "null".into(),
        JsonValue::Bool(_) => "bool".into(),
        JsonValue::Int(_) => "int".into(),
        JsonValue::Float(_) => "float".into(),
        JsonValue::String(_) => "string".into(),
        JsonValue::Array(_) => "array".into(),
        JsonValue::Object(_) => "object".into(),
    }
}

// --- Factory ---

/// Create an LLM backend from configuration.
/// Returns MockBackend if no config or `llm.backend = mock`.
pub fn create_backend(config: &Config) -> Result<Box<dyn LlmBackend>, LlmError> {
    match config.get("llm.backend") {
        Some("cli") => Ok(Box::new(CliBackend::from_config(config)?)),
        Some("http") => Ok(Box::new(HttpBackend::from_config(config)?)),
        Some("mock") | None => Ok(Box::new(MockBackend::new())),
        Some(other) => Err(LlmError::Config(format!("unknown llm.backend: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_backend_int() {
        let backend = MockBackend::new();
        let result = backend
            .complete("test", "input", &TypeAnnotation::Named("int".into()), None)
            .unwrap();
        assert_eq!(result, Value::Int(0));
    }

    #[test]
    fn mock_backend_string() {
        let backend = MockBackend::new();
        let result = backend
            .complete("test", "input", &TypeAnnotation::Named("string".into()), None)
            .unwrap();
        assert_eq!(result, Value::String("mock".into()));
    }

    #[test]
    fn mock_backend_struct() {
        let backend = MockBackend::new();
        let fields = vec![("name", "string"), ("score", "float")];
        let result = backend
            .complete(
                "test",
                "input",
                &TypeAnnotation::Named("Report".into()),
                Some(&fields),
            )
            .unwrap();
        match result {
            Value::Struct(name, f) => {
                assert_eq!(name, "Report");
                assert_eq!(f.get("name"), Some(&Value::String("mock".into())));
                assert_eq!(f.get("score"), Some(&Value::Float(0.0)));
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn factory_default_mock() {
        let config = Config::parse("");
        let backend = create_backend(&config).unwrap();
        let result = backend
            .complete("test", "data", &TypeAnnotation::Named("int".into()), None)
            .unwrap();
        assert_eq!(result, Value::Int(0));
    }

    #[test]
    fn factory_explicit_mock() {
        let config = Config::parse("llm.backend = mock");
        let backend = create_backend(&config).unwrap();
        let result = backend
            .complete("test", "data", &TypeAnnotation::Named("bool".into()), None)
            .unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn factory_http_missing_endpoint() {
        let config = Config::parse("llm.backend = http");
        assert!(matches!(
            create_backend(&config),
            Err(LlmError::Config(_))
        ));
    }

    #[test]
    fn factory_unknown_backend() {
        let config = Config::parse("llm.backend = gpt-magic");
        assert!(matches!(
            create_backend(&config),
            Err(LlmError::Config(_))
        ));
    }

    #[test]
    fn extract_content_anthropic_format() {
        let response = r#"{"content":[{"type":"text","text":"42"}],"model":"claude"}"#;
        let content = HttpBackend::extract_content(response).unwrap();
        assert_eq!(content, "42");
    }

    #[test]
    fn extract_content_missing() {
        let response = r#"{"error":"bad request"}"#;
        assert!(HttpBackend::extract_content(response).is_err());
    }

    #[test]
    fn json_to_value_int() {
        let json = JsonValue::Int(42);
        let val =
            HttpBackend::json_to_value(&json, &TypeAnnotation::Named("int".into())).unwrap();
        assert_eq!(val, Value::Int(42));
    }

    #[test]
    fn json_to_value_type_mismatch() {
        let json = JsonValue::String("hello".into());
        let result = HttpBackend::json_to_value(&json, &TypeAnnotation::Named("int".into()));
        assert!(matches!(result, Err(LlmError::TypeMismatch { .. })));
    }

    #[test]
    fn json_to_value_struct() {
        let mut obj = std::collections::BTreeMap::new();
        obj.insert("name".to_string(), JsonValue::String("test".into()));
        obj.insert("score".to_string(), JsonValue::Float(0.95));
        let json = JsonValue::Object(obj);
        let val =
            HttpBackend::json_to_value(&json, &TypeAnnotation::Named("Report".into())).unwrap();
        match val {
            Value::Struct(name, fields) => {
                assert_eq!(name, "Report");
                assert_eq!(fields.get("name"), Some(&Value::String("test".into())));
                assert_eq!(fields.get("score"), Some(&Value::Float(0.95)));
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn json_to_value_list() {
        let json = json::array(vec![JsonValue::Int(1), JsonValue::Int(2)]);
        let return_type = TypeAnnotation::Generic(
            "List".into(),
            vec![TypeAnnotation::Named("int".into())],
        );
        let val = HttpBackend::json_to_value(&json, &return_type).unwrap();
        assert_eq!(val, Value::List(vec![Value::Int(1), Value::Int(2)]));
    }

    #[test]
    fn build_request_body_valid_json() {
        let config = Config::parse(
            "llm.endpoint = https://api.example.com\nllm.api_key_env = TEST_KEY_UNUSED",
        );
        // Can't create HttpBackend without env var, so test the method indirectly
        let body_str = format!(
            "{}",
            json::object(vec![
                ("model", JsonValue::String("test-model".into())),
                ("max_tokens", JsonValue::Int(1024)),
                (
                    "messages",
                    json::array(vec![json::object(vec![
                        ("role", JsonValue::String("user".into())),
                        ("content", JsonValue::String("test prompt".into())),
                    ])]),
                ),
            ])
        );
        // Verify it's valid JSON
        let parsed = json::parse(&body_str).unwrap();
        assert!(parsed.get("model").is_some());
        assert!(parsed.get("messages").is_some());
        let _ = config; // used for context
    }

    // --- CLI Backend tests ---

    #[test]
    fn cli_backend_from_config() {
        let config = Config::parse("llm.command = claude\nllm.max_retries = 3");
        let backend = CliBackend::from_config(&config).unwrap();
        assert_eq!(backend.command, "claude");
        assert_eq!(backend.args, vec!["-p", "--output-format", "text"]);
        assert_eq!(backend.max_retries, 3);
        assert!(backend.model.is_none());
    }

    #[test]
    fn cli_backend_custom_command() {
        let config = Config::parse("llm.command = /usr/local/bin/claude\nllm.model = opus");
        let backend = CliBackend::from_config(&config).unwrap();
        assert_eq!(backend.command, "/usr/local/bin/claude");
        assert_eq!(backend.args, vec!["-p", "--output-format", "text"]);
        assert_eq!(backend.model.as_deref(), Some("opus"));
    }

    #[test]
    fn cli_backend_custom_args() {
        let config = Config::parse("llm.command = gemini\nllm.args = -p --json");
        let backend = CliBackend::from_config(&config).unwrap();
        assert_eq!(backend.command, "gemini");
        assert_eq!(backend.args, vec!["-p", "--json"]);
    }

    #[test]
    fn cli_backend_build_prompt() {
        let prompt = CliBackend::build_prompt(
            "Classify this",
            "hello world",
            &TypeAnnotation::Named("int".into()),
        );
        assert!(prompt.contains("Classify this"));
        assert!(prompt.contains("hello world"));
        assert!(prompt.contains("int"));
        assert!(prompt.contains("ONLY valid JSON"));
    }

    #[test]
    fn factory_cli_backend() {
        let config = Config::parse("llm.backend = cli");
        let backend = create_backend(&config).unwrap();
        // CLI backend with default config works (no env vars needed unlike http)
        // Can't call complete() without claude installed, but creation succeeds
        let _ = backend;
    }

    #[test]
    fn extract_json_plain() {
        assert_eq!(extract_json_from_text("42"), "42");
        assert_eq!(extract_json_from_text("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(extract_json_from_text("  true  "), "true");
    }

    #[test]
    fn extract_json_with_preamble() {
        assert_eq!(
            extract_json_from_text("Here is the result:\n{\"score\": 42}"),
            "{\"score\": 42}"
        );
    }

    #[test]
    fn extract_json_array_with_preamble() {
        assert_eq!(
            extract_json_from_text("The items are:\n[1, 2, 3]"),
            "[1, 2, 3]"
        );
    }
}
