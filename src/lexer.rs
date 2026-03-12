/// Token types for the Agentis language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords
    Fn,
    Let,
    If,
    Else,
    Return,
    True,
    False,
    Type,

    // AI-native keywords
    Agent,
    Prompt,
    Validate,
    Explore,
    Cb,
    Import,
    As,
    Spawn,

    // Type keywords
    Int,
    Float,
    String,
    Bool,
    List,
    Map,

    // Literals
    Identifier(std::string::String),
    IntLiteral(i64),
    FloatLiteral(f64),
    StringLiteral(std::string::String),

    // Operators
    Plus,   // +
    Minus,  // -
    Star,   // *
    Slash,  // /
    Assign, // =
    Eq,     // ==
    NotEq,  // !=
    Lt,     // <
    Gt,     // >
    LtEq,   // <=
    GtEq,   // >=
    Arrow,  // ->
    Dot,    // .
    Bang,   // !

    // Delimiters
    LParen,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    Comma,     // ,
    Semicolon, // ;
    Colon,     // :

    // Special
    Eof,
}

/// A token with its position in the source code.
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub line: usize,
    pub column: usize,
}

/// Lexer error with position information.
#[derive(Debug, Clone, PartialEq)]
pub struct LexerError {
    pub message: std::string::String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for LexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] {}", self.line, self.column, self.message)
    }
}

pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<SpannedToken>, LexerError> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token()?;
            let is_eof = token.token == Token::Eof;
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<SpannedToken, LexerError> {
        self.skip_whitespace_and_comments();

        if self.is_at_end() {
            return Ok(self.make_token(Token::Eof));
        }

        let ch = self.current();

        // String literals
        if ch == '"' {
            return self.read_string();
        }

        // Numbers
        if ch.is_ascii_digit() {
            return self.read_number();
        }

        // Identifiers and keywords
        if ch.is_ascii_alphabetic() || ch == '_' {
            return Ok(self.read_identifier());
        }

        // Operators and delimiters
        self.read_operator_or_delimiter()
    }

    fn skip_whitespace_and_comments(&mut self) {
        while !self.is_at_end() {
            let ch = self.current();
            if ch.is_ascii_whitespace() {
                self.advance();
            } else if ch == '/' && self.peek() == Some('/') {
                // Line comment — skip until end of line
                while !self.is_at_end() && self.current() != '\n' {
                    self.advance();
                }
            } else {
                break;
            }
        }
    }

    fn read_string(&mut self) -> Result<SpannedToken, LexerError> {
        let start_line = self.line;
        let start_col = self.column;
        self.advance(); // skip opening "

        let mut value = std::string::String::new();
        while !self.is_at_end() && self.current() != '"' {
            if self.current() == '\n' {
                return Err(LexerError {
                    message: "unterminated string literal".into(),
                    line: start_line,
                    column: start_col,
                });
            }
            if self.current() == '\\' {
                self.advance();
                if self.is_at_end() {
                    return Err(LexerError {
                        message: "unterminated escape sequence".into(),
                        line: self.line,
                        column: self.column,
                    });
                }
                match self.current() {
                    'n' => value.push('\n'),
                    't' => value.push('\t'),
                    'r' => value.push('\r'),
                    '\\' => value.push('\\'),
                    '"' => value.push('"'),
                    other => {
                        return Err(LexerError {
                            message: format!("unknown escape sequence: \\{other}"),
                            line: self.line,
                            column: self.column,
                        });
                    }
                }
            } else {
                value.push(self.current());
            }
            self.advance();
        }

        if self.is_at_end() {
            return Err(LexerError {
                message: "unterminated string literal".into(),
                line: start_line,
                column: start_col,
            });
        }

        self.advance(); // skip closing "
        Ok(SpannedToken {
            token: Token::StringLiteral(value),
            line: start_line,
            column: start_col,
        })
    }

    fn read_number(&mut self) -> Result<SpannedToken, LexerError> {
        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        while !self.is_at_end() && self.current().is_ascii_digit() {
            self.advance();
        }

        // Check for float
        if !self.is_at_end()
            && self.current() == '.'
            && self.peek().is_some_and(|c| c.is_ascii_digit())
        {
            self.advance(); // skip .
            while !self.is_at_end() && self.current().is_ascii_digit() {
                self.advance();
            }
            let text: std::string::String = self.source[start_pos..self.pos].iter().collect();
            let value: f64 = text.parse().map_err(|_| LexerError {
                message: format!("invalid float literal: {text}"),
                line: start_line,
                column: start_col,
            })?;
            return Ok(SpannedToken {
                token: Token::FloatLiteral(value),
                line: start_line,
                column: start_col,
            });
        }

        let text: std::string::String = self.source[start_pos..self.pos].iter().collect();
        let value: i64 = text.parse().map_err(|_| LexerError {
            message: format!("invalid integer literal: {text}"),
            line: start_line,
            column: start_col,
        })?;
        Ok(SpannedToken {
            token: Token::IntLiteral(value),
            line: start_line,
            column: start_col,
        })
    }

    fn read_identifier(&mut self) -> SpannedToken {
        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        while !self.is_at_end() && (self.current().is_ascii_alphanumeric() || self.current() == '_')
        {
            self.advance();
        }

        let text: std::string::String = self.source[start_pos..self.pos].iter().collect();
        let token = match text.as_str() {
            // Keywords
            "fn" => Token::Fn,
            "let" => Token::Let,
            "if" => Token::If,
            "else" => Token::Else,
            "return" => Token::Return,
            "true" => Token::True,
            "false" => Token::False,
            "type" => Token::Type,
            // AI-native keywords
            "agent" => Token::Agent,
            "prompt" => Token::Prompt,
            "validate" => Token::Validate,
            "explore" => Token::Explore,
            "cb" => Token::Cb,
            "import" => Token::Import,
            "as" => Token::As,
            "spawn" => Token::Spawn,
            // Type keywords
            "int" => Token::Int,
            "float" => Token::Float,
            "string" => Token::String,
            "bool" => Token::Bool,
            "list" => Token::List,
            "map" => Token::Map,
            // Identifier
            _ => Token::Identifier(text),
        };

        SpannedToken {
            token,
            line: start_line,
            column: start_col,
        }
    }

    fn read_operator_or_delimiter(&mut self) -> Result<SpannedToken, LexerError> {
        let start_line = self.line;
        let start_col = self.column;
        let ch = self.current();
        self.advance();

        let token = match ch {
            '+' => Token::Plus,
            '*' => Token::Star,
            '.' => Token::Dot,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            ',' => Token::Comma,
            ';' => Token::Semicolon,
            ':' => Token::Colon,
            '/' => Token::Slash,
            '-' => {
                if self.match_char('>') {
                    Token::Arrow
                } else {
                    Token::Minus
                }
            }
            '=' => {
                if self.match_char('=') {
                    Token::Eq
                } else {
                    Token::Assign
                }
            }
            '!' => {
                if self.match_char('=') {
                    Token::NotEq
                } else {
                    Token::Bang
                }
            }
            '<' => {
                if self.match_char('=') {
                    Token::LtEq
                } else {
                    Token::Lt
                }
            }
            '>' => {
                if self.match_char('=') {
                    Token::GtEq
                } else {
                    Token::Gt
                }
            }
            _ => {
                return Err(LexerError {
                    message: format!("unexpected character: '{ch}'"),
                    line: start_line,
                    column: start_col,
                });
            }
        };

        Ok(SpannedToken {
            token,
            line: start_line,
            column: start_col,
        })
    }

    // --- Helper methods ---

    fn current(&self) -> char {
        self.source[self.pos]
    }

    fn peek(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn advance(&mut self) {
        if !self.is_at_end() {
            if self.current() == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.pos += 1;
        }
    }

    fn match_char(&mut self, expected: char) -> bool {
        if !self.is_at_end() && self.current() == expected {
            self.advance();
            true
        } else {
            false
        }
    }

    fn make_token(&self, token: Token) -> SpannedToken {
        SpannedToken {
            token,
            line: self.line,
            column: self.column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(source: &str) -> Vec<Token> {
        Lexer::new(source)
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|st| st.token)
            .collect()
    }

    #[test]
    fn empty_source() {
        assert_eq!(tokenize(""), vec![Token::Eof]);
    }

    #[test]
    fn whitespace_only() {
        assert_eq!(tokenize("   \n\t  \n  "), vec![Token::Eof]);
    }

    #[test]
    fn language_keywords() {
        assert_eq!(
            tokenize("fn let if else return true false type"),
            vec![
                Token::Fn,
                Token::Let,
                Token::If,
                Token::Else,
                Token::Return,
                Token::True,
                Token::False,
                Token::Type,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn ai_native_keywords() {
        assert_eq!(
            tokenize("agent prompt validate explore cb"),
            vec![
                Token::Agent,
                Token::Prompt,
                Token::Validate,
                Token::Explore,
                Token::Cb,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn type_keywords() {
        assert_eq!(
            tokenize("int float string bool list map"),
            vec![
                Token::Int,
                Token::Float,
                Token::String,
                Token::Bool,
                Token::List,
                Token::Map,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn identifiers() {
        assert_eq!(
            tokenize("foo bar_baz _private x1"),
            vec![
                Token::Identifier("foo".into()),
                Token::Identifier("bar_baz".into()),
                Token::Identifier("_private".into()),
                Token::Identifier("x1".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn integer_literals() {
        assert_eq!(
            tokenize("0 42 1000"),
            vec![
                Token::IntLiteral(0),
                Token::IntLiteral(42),
                Token::IntLiteral(1000),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn float_literals() {
        assert_eq!(
            tokenize("3.14 0.5 100.0"),
            vec![
                Token::FloatLiteral(3.14),
                Token::FloatLiteral(0.5),
                Token::FloatLiteral(100.0),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn dot_not_float() {
        // `x.y` should be identifier, dot, identifier — not a float
        assert_eq!(
            tokenize("x.y"),
            vec![
                Token::Identifier("x".into()),
                Token::Dot,
                Token::Identifier("y".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn string_literals() {
        assert_eq!(
            tokenize(r#""hello" "world""#),
            vec![
                Token::StringLiteral("hello".into()),
                Token::StringLiteral("world".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn string_escape_sequences() {
        assert_eq!(
            tokenize(r#""\n\t\\\"""#),
            vec![Token::StringLiteral("\n\t\\\"".into()), Token::Eof,]
        );
    }

    #[test]
    fn unterminated_string() {
        let result = Lexer::new(r#""hello"#).tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("unterminated"));
    }

    #[test]
    fn operators() {
        assert_eq!(
            tokenize("+ - * / = == != < > <= >= -> ."),
            vec![
                Token::Plus,
                Token::Minus,
                Token::Star,
                Token::Slash,
                Token::Assign,
                Token::Eq,
                Token::NotEq,
                Token::Lt,
                Token::Gt,
                Token::LtEq,
                Token::GtEq,
                Token::Arrow,
                Token::Dot,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn bang_standalone() {
        assert_eq!(tokenize("!"), vec![Token::Bang, Token::Eof]);
    }

    #[test]
    fn delimiters() {
        assert_eq!(
            tokenize("( ) { } , ; :"),
            vec![
                Token::LParen,
                Token::RParen,
                Token::LBrace,
                Token::RBrace,
                Token::Comma,
                Token::Semicolon,
                Token::Colon,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn line_comments() {
        assert_eq!(
            tokenize("fn // this is a comment\nlet"),
            vec![Token::Fn, Token::Let, Token::Eof]
        );
    }

    #[test]
    fn comment_at_eof() {
        assert_eq!(
            tokenize("fn // trailing comment"),
            vec![Token::Fn, Token::Eof]
        );
    }

    #[test]
    fn unexpected_character() {
        let result = Lexer::new("@").tokenize();
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("unexpected character"));
    }

    #[test]
    fn position_tracking() {
        let tokens = Lexer::new("fn\n  let").tokenize().unwrap();
        assert_eq!(tokens[0].line, 1);
        assert_eq!(tokens[0].column, 1);
        assert_eq!(tokens[1].line, 2);
        assert_eq!(tokens[1].column, 3);
    }

    #[test]
    fn agent_declaration() {
        assert_eq!(
            tokenize("agent scanner(url: string) -> Report {"),
            vec![
                Token::Agent,
                Token::Identifier("scanner".into()),
                Token::LParen,
                Token::Identifier("url".into()),
                Token::Colon,
                Token::String,
                Token::RParen,
                Token::Arrow,
                Token::Identifier("Report".into()),
                Token::LBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn prompt_expression() {
        assert_eq!(
            tokenize(r#"prompt("Classify this", input) -> Category"#),
            vec![
                Token::Prompt,
                Token::LParen,
                Token::StringLiteral("Classify this".into()),
                Token::Comma,
                Token::Identifier("input".into()),
                Token::RParen,
                Token::Arrow,
                Token::Identifier("Category".into()),
                Token::Eof,
            ]
        );
    }

    #[test]
    fn validate_block() {
        assert_eq!(
            tokenize("validate result { result.confidence > 0.8 }"),
            vec![
                Token::Validate,
                Token::Identifier("result".into()),
                Token::LBrace,
                Token::Identifier("result".into()),
                Token::Dot,
                Token::Identifier("confidence".into()),
                Token::Gt,
                Token::FloatLiteral(0.8),
                Token::RBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn explore_block() {
        assert_eq!(
            tokenize(r#"explore "feature-name" {"#),
            vec![
                Token::Explore,
                Token::StringLiteral("feature-name".into()),
                Token::LBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn cognitive_budget_statement() {
        assert_eq!(
            tokenize("cb 1000;"),
            vec![
                Token::Cb,
                Token::IntLiteral(1000),
                Token::Semicolon,
                Token::Eof
            ]
        );
    }

    #[test]
    fn full_function() {
        let source = r#"
fn add(a: int, b: int) -> int {
    return a + b;
}
"#;
        assert_eq!(
            tokenize(source),
            vec![
                Token::Fn,
                Token::Identifier("add".into()),
                Token::LParen,
                Token::Identifier("a".into()),
                Token::Colon,
                Token::Int,
                Token::Comma,
                Token::Identifier("b".into()),
                Token::Colon,
                Token::Int,
                Token::RParen,
                Token::Arrow,
                Token::Int,
                Token::LBrace,
                Token::Return,
                Token::Identifier("a".into()),
                Token::Plus,
                Token::Identifier("b".into()),
                Token::Semicolon,
                Token::RBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn type_declaration() {
        let source = r#"
type Category {
    label: string,
    confidence: float
}
"#;
        assert_eq!(
            tokenize(source),
            vec![
                Token::Type,
                Token::Identifier("Category".into()),
                Token::LBrace,
                Token::Identifier("label".into()),
                Token::Colon,
                Token::String,
                Token::Comma,
                Token::Identifier("confidence".into()),
                Token::Colon,
                Token::Float,
                Token::RBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn generic_types() {
        assert_eq!(
            tokenize("list<int>"),
            vec![Token::List, Token::Lt, Token::Int, Token::Gt, Token::Eof,]
        );
    }

    #[test]
    fn multiline_newline_in_string() {
        let result = Lexer::new("\"hello\nworld\"").tokenize();
        assert!(result.is_err());
    }
}
