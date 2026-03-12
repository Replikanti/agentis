// Minimal JSON builder and parser for Agentis.
//
// Covers: strings (with escaping), numbers (int/float), booleans, null,
// arrays, objects. No streaming, no comments, no trailing commas.
//
// Used by: LLM request/response handling, config metadata.

use std::collections::BTreeMap;

// --- JSON Value ---

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsonValue::Float(n) => Some(*n),
            JsonValue::Int(n) => Some(*n as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, JsonValue>> {
        match self {
            JsonValue::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsonValue> {
        self.as_object()?.get(key)
    }
}

// --- JSON Builder ---

impl std::fmt::Display for JsonValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonValue::Null => write!(f, "null"),
            JsonValue::Bool(b) => write!(f, "{}", if *b { "true" } else { "false" }),
            JsonValue::Int(n) => write!(f, "{n}"),
            JsonValue::Float(n) => {
                if n.fract() == 0.0 {
                    write!(f, "{n}.0")
                } else {
                    write!(f, "{n}")
                }
            }
            JsonValue::String(s) => write_json_string(f, s),
            JsonValue::Array(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            JsonValue::Object(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write_json_string(f, k)?;
                    write!(f, ":{v}")?;
                }
                write!(f, "}}")
            }
        }
    }
}

fn write_json_string(f: &mut std::fmt::Formatter<'_>, s: &str) -> std::fmt::Result {
    write!(f, "\"")?;
    for ch in s.chars() {
        match ch {
            '"' => write!(f, "\\\"")?,
            '\\' => write!(f, "\\\\")?,
            '\n' => write!(f, "\\n")?,
            '\r' => write!(f, "\\r")?,
            '\t' => write!(f, "\\t")?,
            c if c < '\x20' => write!(f, "\\u{:04x}", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    write!(f, "\"")
}

/// Build a JSON object from key-value pairs.
pub fn object(pairs: Vec<(&str, JsonValue)>) -> JsonValue {
    let mut map = BTreeMap::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v);
    }
    JsonValue::Object(map)
}

/// Build a JSON array.
pub fn array(items: Vec<JsonValue>) -> JsonValue {
    JsonValue::Array(items)
}

// --- JSON Parser ---

#[derive(Debug, Clone, PartialEq)]
pub struct JsonError {
    pub message: String,
    pub position: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JSON error at position {}: {}",
            self.position, self.message
        )
    }
}

pub fn parse(input: &str) -> Result<JsonValue, JsonError> {
    let mut pos = 0;
    let bytes = input.as_bytes();
    skip_whitespace(bytes, &mut pos);
    let value = parse_value(bytes, &mut pos)?;
    skip_whitespace(bytes, &mut pos);
    if pos < bytes.len() {
        return Err(JsonError {
            message: "trailing data after JSON value".into(),
            position: pos,
        });
    }
    Ok(value)
}

fn parse_value(bytes: &[u8], pos: &mut usize) -> Result<JsonValue, JsonError> {
    skip_whitespace(bytes, pos);
    if *pos >= bytes.len() {
        return Err(JsonError {
            message: "unexpected end of input".into(),
            position: *pos,
        });
    }
    match bytes[*pos] {
        b'"' => parse_string(bytes, pos).map(JsonValue::String),
        b'{' => parse_object(bytes, pos),
        b'[' => parse_array(bytes, pos),
        b't' => parse_literal(bytes, pos, b"true", JsonValue::Bool(true)),
        b'f' => parse_literal(bytes, pos, b"false", JsonValue::Bool(false)),
        b'n' => parse_literal(bytes, pos, b"null", JsonValue::Null),
        b'-' | b'0'..=b'9' => parse_number(bytes, pos),
        ch => Err(JsonError {
            message: format!("unexpected character: '{}'", ch as char),
            position: *pos,
        }),
    }
}

fn parse_string(bytes: &[u8], pos: &mut usize) -> Result<String, JsonError> {
    if bytes[*pos] != b'"' {
        return Err(JsonError {
            message: "expected '\"'".into(),
            position: *pos,
        });
    }
    *pos += 1;
    let mut result = String::new();
    loop {
        if *pos >= bytes.len() {
            return Err(JsonError {
                message: "unterminated string".into(),
                position: *pos,
            });
        }
        match bytes[*pos] {
            b'"' => {
                *pos += 1;
                return Ok(result);
            }
            b'\\' => {
                *pos += 1;
                if *pos >= bytes.len() {
                    return Err(JsonError {
                        message: "unterminated escape".into(),
                        position: *pos,
                    });
                }
                match bytes[*pos] {
                    b'"' => result.push('"'),
                    b'\\' => result.push('\\'),
                    b'/' => result.push('/'),
                    b'n' => result.push('\n'),
                    b'r' => result.push('\r'),
                    b't' => result.push('\t'),
                    b'u' => {
                        *pos += 1;
                        let hex = read_hex4(bytes, pos)?;
                        if let Some(ch) = char::from_u32(hex) {
                            result.push(ch);
                        }
                        continue; // don't advance pos again
                    }
                    ch => {
                        return Err(JsonError {
                            message: format!("invalid escape: \\{}", ch as char),
                            position: *pos,
                        });
                    }
                }
                *pos += 1;
            }
            ch => {
                result.push(ch as char);
                *pos += 1;
            }
        }
    }
}

fn read_hex4(bytes: &[u8], pos: &mut usize) -> Result<u32, JsonError> {
    if *pos + 4 > bytes.len() {
        return Err(JsonError {
            message: "truncated unicode escape".into(),
            position: *pos,
        });
    }
    let hex_str = std::str::from_utf8(&bytes[*pos..*pos + 4]).map_err(|_| JsonError {
        message: "invalid unicode escape".into(),
        position: *pos,
    })?;
    let val = u32::from_str_radix(hex_str, 16).map_err(|_| JsonError {
        message: format!("invalid hex: {hex_str}"),
        position: *pos,
    })?;
    *pos += 4;
    Ok(val)
}

fn parse_number(bytes: &[u8], pos: &mut usize) -> Result<JsonValue, JsonError> {
    let start = *pos;
    let mut is_float = false;

    if *pos < bytes.len() && bytes[*pos] == b'-' {
        *pos += 1;
    }

    while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }

    if *pos < bytes.len() && bytes[*pos] == b'.' {
        is_float = true;
        *pos += 1;
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }

    if *pos < bytes.len() && (bytes[*pos] == b'e' || bytes[*pos] == b'E') {
        is_float = true;
        *pos += 1;
        if *pos < bytes.len() && (bytes[*pos] == b'+' || bytes[*pos] == b'-') {
            *pos += 1;
        }
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }

    let num_str = std::str::from_utf8(&bytes[start..*pos]).map_err(|_| JsonError {
        message: "invalid number".into(),
        position: start,
    })?;

    if is_float {
        let n: f64 = num_str.parse().map_err(|_| JsonError {
            message: format!("invalid float: {num_str}"),
            position: start,
        })?;
        Ok(JsonValue::Float(n))
    } else {
        let n: i64 = num_str.parse().map_err(|_| JsonError {
            message: format!("invalid integer: {num_str}"),
            position: start,
        })?;
        Ok(JsonValue::Int(n))
    }
}

fn parse_object(bytes: &[u8], pos: &mut usize) -> Result<JsonValue, JsonError> {
    *pos += 1; // skip '{'
    let mut map = BTreeMap::new();
    skip_whitespace(bytes, pos);
    if *pos < bytes.len() && bytes[*pos] == b'}' {
        *pos += 1;
        return Ok(JsonValue::Object(map));
    }
    loop {
        skip_whitespace(bytes, pos);
        let key = parse_string(bytes, pos)?;
        skip_whitespace(bytes, pos);
        expect_byte(bytes, pos, b':')?;
        let value = parse_value(bytes, pos)?;
        map.insert(key, value);
        skip_whitespace(bytes, pos);
        if *pos >= bytes.len() {
            return Err(JsonError {
                message: "unterminated object".into(),
                position: *pos,
            });
        }
        if bytes[*pos] == b'}' {
            *pos += 1;
            return Ok(JsonValue::Object(map));
        }
        expect_byte(bytes, pos, b',')?;
    }
}

fn parse_array(bytes: &[u8], pos: &mut usize) -> Result<JsonValue, JsonError> {
    *pos += 1; // skip '['
    let mut items = Vec::new();
    skip_whitespace(bytes, pos);
    if *pos < bytes.len() && bytes[*pos] == b']' {
        *pos += 1;
        return Ok(JsonValue::Array(items));
    }
    loop {
        items.push(parse_value(bytes, pos)?);
        skip_whitespace(bytes, pos);
        if *pos >= bytes.len() {
            return Err(JsonError {
                message: "unterminated array".into(),
                position: *pos,
            });
        }
        if bytes[*pos] == b']' {
            *pos += 1;
            return Ok(JsonValue::Array(items));
        }
        expect_byte(bytes, pos, b',')?;
    }
}

fn parse_literal(
    bytes: &[u8],
    pos: &mut usize,
    expected: &[u8],
    value: JsonValue,
) -> Result<JsonValue, JsonError> {
    if bytes[*pos..].starts_with(expected) {
        *pos += expected.len();
        Ok(value)
    } else {
        Err(JsonError {
            message: format!(
                "expected '{}'",
                std::str::from_utf8(expected).unwrap_or("?")
            ),
            position: *pos,
        })
    }
}

fn skip_whitespace(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

fn expect_byte(bytes: &[u8], pos: &mut usize, expected: u8) -> Result<(), JsonError> {
    if *pos >= bytes.len() || bytes[*pos] != expected {
        return Err(JsonError {
            message: format!("expected '{}'", expected as char),
            position: *pos,
        });
    }
    *pos += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Builder ---

    #[test]
    fn build_null() {
        assert_eq!(JsonValue::Null.to_string(), "null");
    }

    #[test]
    fn build_bool() {
        assert_eq!(JsonValue::Bool(true).to_string(), "true");
        assert_eq!(JsonValue::Bool(false).to_string(), "false");
    }

    #[test]
    fn build_int() {
        assert_eq!(JsonValue::Int(42).to_string(), "42");
        assert_eq!(JsonValue::Int(-7).to_string(), "-7");
    }

    #[test]
    fn build_float() {
        assert_eq!(JsonValue::Float(3.14).to_string(), "3.14");
        assert_eq!(JsonValue::Float(1.0).to_string(), "1.0");
    }

    #[test]
    fn build_string_escaping() {
        let val = JsonValue::String("hello \"world\"\nnewline".into());
        assert_eq!(val.to_string(), "\"hello \\\"world\\\"\\nnewline\"");
    }

    #[test]
    fn build_string_control_chars() {
        let val = JsonValue::String("\x01\x1f".into());
        assert_eq!(val.to_string(), "\"\\u0001\\u001f\"");
    }

    #[test]
    fn build_array() {
        let val = array(vec![
            JsonValue::Int(1),
            JsonValue::Int(2),
            JsonValue::Int(3),
        ]);
        assert_eq!(val.to_string(), "[1,2,3]");
    }

    #[test]
    fn build_object() {
        let val = object(vec![
            ("name", JsonValue::String("agentis".into())),
            ("ver", JsonValue::Int(1)),
        ]);
        assert_eq!(val.to_string(), "{\"name\":\"agentis\",\"ver\":1}");
    }

    #[test]
    fn build_nested() {
        let val = object(vec![(
            "items",
            array(vec![JsonValue::Bool(true), JsonValue::Null]),
        )]);
        assert_eq!(val.to_string(), "{\"items\":[true,null]}");
    }

    // --- Parser ---

    #[test]
    fn parse_null() {
        assert_eq!(parse("null").unwrap(), JsonValue::Null);
    }

    #[test]
    fn parse_bool() {
        assert_eq!(parse("true").unwrap(), JsonValue::Bool(true));
        assert_eq!(parse("false").unwrap(), JsonValue::Bool(false));
    }

    #[test]
    fn parse_int() {
        assert_eq!(parse("42").unwrap(), JsonValue::Int(42));
        assert_eq!(parse("-7").unwrap(), JsonValue::Int(-7));
        assert_eq!(parse("0").unwrap(), JsonValue::Int(0));
    }

    #[test]
    fn parse_float() {
        assert_eq!(parse("3.14").unwrap(), JsonValue::Float(3.14));
        assert_eq!(parse("-0.5").unwrap(), JsonValue::Float(-0.5));
        assert_eq!(parse("1e10").unwrap(), JsonValue::Float(1e10));
        assert_eq!(parse("2.5E-3").unwrap(), JsonValue::Float(2.5e-3));
    }

    #[test]
    fn parse_string_simple() {
        assert_eq!(
            parse("\"hello\"").unwrap(),
            JsonValue::String("hello".into())
        );
    }

    #[test]
    fn parse_string_escapes() {
        assert_eq!(
            parse("\"a\\\"b\\\\c\\n\\t\"").unwrap(),
            JsonValue::String("a\"b\\c\n\t".into())
        );
    }

    #[test]
    fn parse_string_unicode() {
        assert_eq!(parse("\"\\u0041\"").unwrap(), JsonValue::String("A".into()));
    }

    #[test]
    fn parse_empty_array() {
        assert_eq!(parse("[]").unwrap(), JsonValue::Array(vec![]));
    }

    #[test]
    fn parse_array_items() {
        assert_eq!(
            parse("[1, 2, 3]").unwrap(),
            JsonValue::Array(vec![
                JsonValue::Int(1),
                JsonValue::Int(2),
                JsonValue::Int(3)
            ])
        );
    }

    #[test]
    fn parse_empty_object() {
        assert_eq!(parse("{}").unwrap(), JsonValue::Object(BTreeMap::new()));
    }

    #[test]
    fn parse_object_entries() {
        let result = parse("{\"a\": 1, \"b\": true}").unwrap();
        let obj = result.as_object().unwrap();
        assert_eq!(obj.get("a"), Some(&JsonValue::Int(1)));
        assert_eq!(obj.get("b"), Some(&JsonValue::Bool(true)));
    }

    #[test]
    fn parse_nested() {
        let result = parse("{\"items\": [1, {\"x\": null}]}").unwrap();
        let items = result.get("items").unwrap().as_array().unwrap();
        assert_eq!(items[0], JsonValue::Int(1));
        assert_eq!(items[1].get("x"), Some(&JsonValue::Null));
    }

    #[test]
    fn parse_whitespace_tolerance() {
        assert_eq!(parse("  42  ").unwrap(), JsonValue::Int(42));
        assert_eq!(
            parse("  {  \"a\"  :  1  }  ").unwrap().get("a"),
            Some(&JsonValue::Int(1))
        );
    }

    // --- Round-trip ---

    #[test]
    fn round_trip() {
        let val = object(vec![
            ("msg", JsonValue::String("hello \"world\"".into())),
            (
                "nums",
                array(vec![JsonValue::Int(1), JsonValue::Float(2.5)]),
            ),
            ("ok", JsonValue::Bool(true)),
            ("nil", JsonValue::Null),
        ]);
        let json_str = val.to_string();
        let parsed = parse(&json_str).unwrap();
        assert_eq!(parsed, val);
    }

    // --- Error cases ---

    #[test]
    fn error_trailing_data() {
        assert!(parse("42 extra").is_err());
    }

    #[test]
    fn error_unterminated_string() {
        assert!(parse("\"hello").is_err());
    }

    #[test]
    fn error_unterminated_object() {
        assert!(parse("{\"a\": 1").is_err());
    }

    #[test]
    fn error_empty_input() {
        assert!(parse("").is_err());
    }

    #[test]
    fn error_invalid_escape() {
        assert!(parse("\"\\q\"").is_err());
    }

    // --- Accessor helpers ---

    #[test]
    fn accessor_methods() {
        assert_eq!(JsonValue::String("hi".into()).as_str(), Some("hi"));
        assert_eq!(JsonValue::Int(5).as_i64(), Some(5));
        assert_eq!(JsonValue::Float(1.5).as_f64(), Some(1.5));
        assert_eq!(JsonValue::Int(3).as_f64(), Some(3.0));
        assert_eq!(JsonValue::Bool(true).as_bool(), Some(true));
        assert_eq!(JsonValue::Null.as_str(), None);
    }
}
