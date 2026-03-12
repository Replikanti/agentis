use crate::ast::*;
use crate::lexer::{Lexer, SpannedToken, Token};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] {}", self.line, self.column, self.message)
    }
}

pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
    errors: Vec<ParseError>,
}

impl Parser {
    pub fn new(tokens: Vec<SpannedToken>) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    pub fn parse_source(source: &str) -> Result<Program, ParseError> {
        let tokens = Lexer::new(source).tokenize().map_err(|e| ParseError {
            message: e.message,
            line: e.line,
            column: e.column,
        })?;
        let mut parser = Parser::new(tokens);
        parser.parse_program()
    }

    /// Parse and return all collected errors (for multi-error reporting).
    pub fn parse_source_multi(source: &str) -> Result<Program, Vec<ParseError>> {
        let tokens = Lexer::new(source).tokenize().map_err(|e| {
            vec![ParseError {
                message: e.message,
                line: e.line,
                column: e.column,
            }]
        })?;
        let mut parser = Parser::new(tokens);
        let program = parser.parse_program_recovering();
        if parser.errors.is_empty() {
            Ok(program)
        } else {
            Err(parser.errors)
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, ParseError> {
        let mut declarations = Vec::new();
        while !self.is_at_end() {
            declarations.push(self.parse_declaration()?);
        }
        Ok(Program { declarations })
    }

    fn parse_program_recovering(&mut self) -> Program {
        let mut declarations = Vec::new();
        while !self.is_at_end() {
            match self.parse_declaration() {
                Ok(decl) => declarations.push(decl),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
        }
        Program { declarations }
    }

    fn synchronize(&mut self) {
        self.advance();
        while !self.is_at_end() {
            // Stop at statement boundaries
            match self.current_token() {
                Token::Fn
                | Token::Agent
                | Token::Type
                | Token::Import
                | Token::Let
                | Token::Return => return,
                Token::Semicolon => {
                    self.advance();
                    return;
                }
                _ => self.advance(),
            }
        }
    }

    /// Parse a single REPL input line (may be a declaration, statement, or bare expression).
    /// Bare expressions without trailing `;` are accepted.
    pub fn parse_repl_input(source: &str) -> Result<Declaration, ParseError> {
        let tokens = Lexer::new(source).tokenize().map_err(|e| ParseError {
            message: e.message,
            line: e.line,
            column: e.column,
        })?;
        let mut parser = Parser::new(tokens);
        if parser.is_at_end() {
            return Ok(Declaration::Statement(Statement::Expression(ExprStmt {
                expr: Expr::Identifier("void".to_string()),
            })));
        }
        let decl = parser.parse_repl_declaration()?;
        Ok(decl)
    }

    /// Like parse_declaration but allows bare expressions without semicolons.
    fn parse_repl_declaration(&mut self) -> Result<Declaration, ParseError> {
        match self.current_token() {
            Token::Fn => Ok(Declaration::Function(self.parse_fn_decl()?)),
            Token::Agent => Ok(Declaration::Agent(self.parse_agent_decl()?)),
            Token::Type => Ok(Declaration::Type(self.parse_type_decl()?)),
            Token::Import => Ok(Declaration::Import(self.parse_import_decl()?)),
            Token::Let => Ok(Declaration::Statement(self.parse_let_stmt()?)),
            Token::Return => Ok(Declaration::Statement(self.parse_return_stmt()?)),
            Token::Cb => Ok(Declaration::Statement(self.parse_cb_stmt()?)),
            _ => {
                // Parse expression; semicolon is optional in REPL
                let expr = self.parse_expression()?;
                if self.current_token() == Token::Semicolon {
                    self.advance();
                }
                Ok(Declaration::Statement(Statement::Expression(ExprStmt {
                    expr,
                })))
            }
        }
    }

    // --- Declarations ---

    fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        match self.current_token() {
            Token::Fn => Ok(Declaration::Function(self.parse_fn_decl()?)),
            Token::Agent => Ok(Declaration::Agent(self.parse_agent_decl()?)),
            Token::Type => Ok(Declaration::Type(self.parse_type_decl()?)),
            Token::Import => Ok(Declaration::Import(self.parse_import_decl()?)),
            _ => Ok(Declaration::Statement(self.parse_statement()?)),
        }
    }

    fn parse_fn_decl(&mut self) -> Result<FnDecl, ParseError> {
        self.expect(Token::Fn)?;
        let name = self.expect_identifier()?;
        let params = self.parse_param_list()?;
        let return_type = self.parse_optional_return_type()?;
        let body = self.parse_block()?;
        Ok(FnDecl {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_agent_decl(&mut self) -> Result<AgentDecl, ParseError> {
        self.expect(Token::Agent)?;
        let name = self.expect_identifier()?;
        let params = self.parse_param_list()?;
        let return_type = self.parse_optional_return_type()?;
        let body = self.parse_block()?;
        Ok(AgentDecl {
            name,
            params,
            return_type,
            body,
        })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParseError> {
        self.expect(Token::Type)?;
        let name = self.expect_identifier()?;
        self.expect(Token::LBrace)?;
        let mut fields = Vec::new();
        while self.current_token() != Token::RBrace {
            let field_name = self.expect_identifier()?;
            self.expect(Token::Colon)?;
            let type_annotation = self.parse_type_annotation()?;
            fields.push(TypeField {
                name: field_name,
                type_annotation,
            });
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        Ok(TypeDecl { name, fields })
    }

    /// Parse `import "hash" as alias;` or `import "hash" { name1, name2 };`
    fn parse_import_decl(&mut self) -> Result<ImportDecl, ParseError> {
        self.expect(Token::Import)?;
        let hash = match self.current_token() {
            Token::StringLiteral(s) => {
                let h = s.clone();
                self.advance();
                h
            }
            other => {
                return Err(self.error(&format!("expected string literal (hash), got {other:?}")));
            }
        };

        let mut alias = None;
        let mut names = None;

        if self.current_token() == Token::As {
            // import "hash" as alias;
            self.advance();
            alias = Some(self.expect_identifier()?);
        } else if self.current_token() == Token::LBrace {
            // import "hash" { name1, name2 };
            self.advance();
            let mut name_list = Vec::new();
            while self.current_token() != Token::RBrace {
                name_list.push(self.expect_identifier()?);
                if self.current_token() == Token::Comma {
                    self.advance();
                }
            }
            self.expect(Token::RBrace)?;
            names = Some(name_list);
        }

        self.expect(Token::Semicolon)?;
        Ok(ImportDecl { hash, alias, names })
    }

    fn parse_param_list(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(Token::LParen)?;
        let mut params = Vec::new();
        while self.current_token() != Token::RParen {
            let name = self.expect_identifier()?;
            self.expect(Token::Colon)?;
            let type_annotation = self.parse_type_annotation()?;
            params.push(Param {
                name,
                type_annotation,
            });
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        Ok(params)
    }

    fn parse_optional_return_type(&mut self) -> Result<Option<TypeAnnotation>, ParseError> {
        if self.current_token() == Token::Arrow {
            self.advance();
            Ok(Some(self.parse_type_annotation()?))
        } else {
            Ok(None)
        }
    }

    fn parse_type_annotation(&mut self) -> Result<TypeAnnotation, ParseError> {
        let name = match self.current_token() {
            Token::Int => {
                self.advance();
                "int".to_string()
            }
            Token::Float => {
                self.advance();
                "float".to_string()
            }
            Token::String => {
                self.advance();
                "string".to_string()
            }
            Token::Bool => {
                self.advance();
                "bool".to_string()
            }
            Token::List => {
                self.advance();
                "list".to_string()
            }
            Token::Map => {
                self.advance();
                "map".to_string()
            }
            Token::Identifier(_) => self.expect_identifier()?,
            _ => return Err(self.error("expected type annotation")),
        };

        // Check for generic parameters: list<int>, map<string, int>
        if self.current_token() == Token::Lt {
            self.advance();
            let mut type_params = Vec::new();
            type_params.push(self.parse_type_annotation()?);
            while self.current_token() == Token::Comma {
                self.advance();
                type_params.push(self.parse_type_annotation()?);
            }
            self.expect(Token::Gt)?;
            Ok(TypeAnnotation::Generic(name, type_params))
        } else {
            Ok(TypeAnnotation::Named(name))
        }
    }

    // --- Statements ---

    fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        match self.current_token() {
            Token::Let => self.parse_let_stmt(),
            Token::Return => self.parse_return_stmt(),
            Token::Cb => self.parse_cb_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_let_stmt(&mut self) -> Result<Statement, ParseError> {
        self.expect(Token::Let)?;
        let name = self.expect_identifier()?;
        let type_annotation = if self.current_token() == Token::Colon {
            self.advance();
            Some(self.parse_type_annotation()?)
        } else {
            None
        };
        self.expect(Token::Assign)?;
        let value = self.parse_expression()?;
        self.expect(Token::Semicolon)?;
        Ok(Statement::Let(LetStmt {
            name,
            type_annotation,
            value,
        }))
    }

    fn parse_return_stmt(&mut self) -> Result<Statement, ParseError> {
        self.expect(Token::Return)?;
        if self.current_token() == Token::Semicolon {
            self.advance();
            return Ok(Statement::Return(ReturnStmt { value: None }));
        }
        let value = self.parse_expression()?;
        self.expect(Token::Semicolon)?;
        Ok(Statement::Return(ReturnStmt { value: Some(value) }))
    }

    fn parse_cb_stmt(&mut self) -> Result<Statement, ParseError> {
        self.expect(Token::Cb)?;
        let budget = match self.current_token() {
            Token::IntLiteral(v) => {
                if v < 0 {
                    return Err(self.error("cognitive budget must be non-negative"));
                }
                let val = v as u64;
                self.advance();
                val
            }
            _ => return Err(self.error("expected integer literal for cognitive budget")),
        };
        self.expect(Token::Semicolon)?;
        Ok(Statement::Cb(CbStmt { budget }))
    }

    fn parse_expr_stmt(&mut self) -> Result<Statement, ParseError> {
        let expr = self.parse_expression()?;
        // Block-terminating expressions (if, explore, validate) don't require semicolons
        let needs_semicolon = !matches!(&expr, Expr::If(_) | Expr::Explore(_) | Expr::Validate(_));
        if needs_semicolon {
            self.expect(Token::Semicolon)?;
        } else if self.current_token() == Token::Semicolon {
            self.advance(); // optional semicolon after block expressions
        }
        Ok(Statement::Expression(ExprStmt { expr }))
    }

    fn parse_block(&mut self) -> Result<Block, ParseError> {
        self.expect(Token::LBrace)?;
        let mut statements = Vec::new();
        while self.current_token() != Token::RBrace {
            statements.push(self.parse_statement()?);
        }
        self.expect(Token::RBrace)?;
        Ok(Block { statements })
    }

    // --- Expressions (Pratt parsing) ---

    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        self.parse_precedence(0)
    }

    fn parse_precedence(&mut self, min_prec: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        loop {
            let (op, prec) = match self.current_token() {
                Token::Eq => (BinaryOp::Eq, 1),
                Token::NotEq => (BinaryOp::NotEq, 1),
                Token::Lt => (BinaryOp::Lt, 2),
                Token::Gt => (BinaryOp::Gt, 2),
                Token::LtEq => (BinaryOp::LtEq, 2),
                Token::GtEq => (BinaryOp::GtEq, 2),
                Token::Plus => (BinaryOp::Add, 3),
                Token::Minus => (BinaryOp::Sub, 3),
                Token::Star => (BinaryOp::Mul, 4),
                Token::Slash => (BinaryOp::Div, 4),
                _ => break,
            };

            if prec < min_prec {
                break;
            }

            self.advance();
            let right = self.parse_precedence(prec + 1)?;
            left = Expr::Binary(Box::new(BinaryExpr { op, left, right }));
        }

        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.current_token() {
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::Unary(Box::new(UnaryExpr {
                    op: UnaryOp::Neg,
                    operand,
                })))
            }
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::Unary(Box::new(UnaryExpr {
                    op: UnaryOp::Not,
                    operand,
                })))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.current_token() {
                Token::Dot => {
                    self.advance();
                    let field = self.expect_identifier()?;
                    expr = Expr::FieldAccess(Box::new(FieldAccessExpr {
                        object: expr,
                        field,
                    }));
                }
                Token::LParen => {
                    // Function call: only valid if expr is an identifier
                    if let Expr::Identifier(callee) = expr {
                        expr = self.parse_call_expr(callee)?;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.current_token() {
            Token::IntLiteral(v) => {
                let val = v;
                self.advance();
                Ok(Expr::IntLiteral(val))
            }
            Token::FloatLiteral(v) => {
                let val = v;
                self.advance();
                Ok(Expr::FloatLiteral(val))
            }
            Token::StringLiteral(s) => {
                let val = s;
                self.advance();
                Ok(Expr::StringLiteral(val))
            }
            Token::True => {
                self.advance();
                Ok(Expr::BoolLiteral(true))
            }
            Token::False => {
                self.advance();
                Ok(Expr::BoolLiteral(false))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expression()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Token::If => self.parse_if_expr(),
            Token::Prompt => self.parse_prompt_expr(),
            Token::Validate => self.parse_validate_expr(),
            Token::Explore => self.parse_explore_expr(),
            Token::LBracket => self.parse_list_literal(),
            Token::Spawn => self.parse_spawn_expr(),
            Token::Identifier(_) => {
                let name = self.expect_identifier()?;
                Ok(Expr::Identifier(name))
            }
            _ => Err(self.error(&format!("unexpected token: {:?}", self.current_token()))),
        }
    }

    fn parse_call_expr(&mut self, callee: String) -> Result<Expr, ParseError> {
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while self.current_token() != Token::RParen {
            args.push(self.parse_expression()?);
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        Ok(Expr::Call(Box::new(CallExpr { callee, args })))
    }

    fn parse_if_expr(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::If)?;
        let condition = self.parse_expression()?;
        let then_block = self.parse_block()?;
        let else_block = if self.current_token() == Token::Else {
            self.advance();
            Some(self.parse_block()?)
        } else {
            None
        };
        Ok(Expr::If(Box::new(IfExpr {
            condition,
            then_block,
            else_block,
        })))
    }

    // prompt("instruction", input) -> Type
    fn parse_prompt_expr(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Prompt)?;
        self.expect(Token::LParen)?;
        let instruction = match self.current_token() {
            Token::StringLiteral(s) => {
                let val = s;
                self.advance();
                val
            }
            _ => return Err(self.error("expected string literal for prompt instruction")),
        };
        self.expect(Token::Comma)?;
        let input = self.parse_expression()?;
        self.expect(Token::RParen)?;
        self.expect(Token::Arrow)?;
        let return_type = self.parse_type_annotation()?;
        Ok(Expr::Prompt(Box::new(PromptExpr {
            instruction,
            input,
            return_type,
        })))
    }

    // validate target { pred1, pred2, ... }
    fn parse_validate_expr(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Validate)?;
        let target = self.parse_postfix()?;
        self.expect(Token::LBrace)?;
        let mut predicates = Vec::new();
        while self.current_token() != Token::RBrace {
            predicates.push(self.parse_expression()?);
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RBrace)?;
        Ok(Expr::Validate(Box::new(ValidateExpr {
            target,
            predicates,
        })))
    }

    // explore "name" { ... }
    fn parse_explore_expr(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Explore)?;
        let name = match self.current_token() {
            Token::StringLiteral(s) => {
                let val = s;
                self.advance();
                val
            }
            _ => return Err(self.error("expected string literal for explore block name")),
        };
        let body = self.parse_block()?;
        Ok(Expr::Explore(Box::new(ExploreBlock { name, body })))
    }

    // [expr, expr, ...]
    fn parse_list_literal(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::LBracket)?;
        let mut items = Vec::new();
        while self.current_token() != Token::RBracket {
            items.push(self.parse_expression()?);
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RBracket)?;
        Ok(Expr::ListLiteral(items))
    }

    /// Parse `spawn agent_name(args...)`
    fn parse_spawn_expr(&mut self) -> Result<Expr, ParseError> {
        self.expect(Token::Spawn)?;
        let agent_name = self.expect_identifier()?;
        self.expect(Token::LParen)?;
        let mut args = Vec::new();
        while self.current_token() != Token::RParen {
            args.push(self.parse_expression()?);
            if self.current_token() == Token::Comma {
                self.advance();
            }
        }
        self.expect(Token::RParen)?;
        Ok(Expr::Spawn(Box::new(SpawnExpr { agent_name, args })))
    }

    // --- Token helpers ---

    fn current_token(&self) -> Token {
        self.tokens
            .get(self.pos)
            .map(|st| st.token.clone())
            .unwrap_or(Token::Eof)
    }

    fn current_span(&self) -> (usize, usize) {
        self.tokens
            .get(self.pos)
            .map(|st| (st.line, st.column))
            .unwrap_or((0, 0))
    }

    fn advance(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.current_token(), Token::Eof)
    }

    fn expect(&mut self, expected: Token) -> Result<(), ParseError> {
        let actual = self.current_token();
        if std::mem::discriminant(&actual) == std::mem::discriminant(&expected) {
            self.advance();
            Ok(())
        } else {
            Err(self.error(&format!("expected {expected:?}, got {actual:?}")))
        }
    }

    fn expect_identifier(&mut self) -> Result<String, ParseError> {
        match self.current_token() {
            Token::Identifier(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(self.error(&format!("expected identifier, got {other:?}"))),
        }
    }

    fn error(&self, message: &str) -> ParseError {
        let (line, column) = self.current_span();
        ParseError {
            message: message.to_string(),
            line,
            column,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Program {
        Parser::parse_source(source).unwrap()
    }

    fn parse_err(source: &str) -> ParseError {
        Parser::parse_source(source).unwrap_err()
    }

    // --- Functions ---

    #[test]
    fn empty_program() {
        let prog = parse("");
        assert_eq!(prog.declarations.len(), 0);
    }

    #[test]
    fn simple_function() {
        let prog = parse("fn add(a: int, b: int) -> int { return a + b; }");
        assert_eq!(prog.declarations.len(), 1);
        match &prog.declarations[0] {
            Declaration::Function(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
                assert_eq!(f.params[0].name, "a");
                assert_eq!(f.params[1].name, "b");
                assert!(f.return_type.is_some());
            }
            _ => panic!("expected function declaration"),
        }
    }

    #[test]
    fn function_no_params_no_return() {
        let prog = parse("fn noop() { }");
        match &prog.declarations[0] {
            Declaration::Function(f) => {
                assert_eq!(f.name, "noop");
                assert!(f.params.is_empty());
                assert!(f.return_type.is_none());
                assert!(f.body.statements.is_empty());
            }
            _ => panic!("expected function declaration"),
        }
    }

    // --- Agents ---

    #[test]
    fn agent_declaration() {
        let prog = parse(
            r#"
            agent scanner(url: string) -> Report {
                cb 1000;
                return url;
            }
        "#,
        );
        match &prog.declarations[0] {
            Declaration::Agent(a) => {
                assert_eq!(a.name, "scanner");
                assert_eq!(a.params.len(), 1);
                assert_eq!(a.params[0].name, "url");
                assert!(a.return_type.is_some());
                assert_eq!(a.body.statements.len(), 2);
            }
            _ => panic!("expected agent declaration"),
        }
    }

    // --- Types ---

    #[test]
    fn type_declaration() {
        let prog = parse(
            r#"
            type Category {
                label: string,
                confidence: float
            }
        "#,
        );
        match &prog.declarations[0] {
            Declaration::Type(t) => {
                assert_eq!(t.name, "Category");
                assert_eq!(t.fields.len(), 2);
                assert_eq!(t.fields[0].name, "label");
                assert_eq!(t.fields[1].name, "confidence");
            }
            _ => panic!("expected type declaration"),
        }
    }

    #[test]
    fn type_with_generic_field() {
        let prog = parse(
            r#"
            type Dataset {
                items: list<string>,
                metadata: map<string, int>
            }
        "#,
        );
        match &prog.declarations[0] {
            Declaration::Type(t) => {
                assert_eq!(t.fields.len(), 2);
                match &t.fields[0].type_annotation {
                    TypeAnnotation::Generic(name, params) => {
                        assert_eq!(name, "list");
                        assert_eq!(params.len(), 1);
                    }
                    _ => panic!("expected generic type"),
                }
            }
            _ => panic!("expected type declaration"),
        }
    }

    // --- Let statements ---

    #[test]
    fn let_with_type() {
        let prog = parse("let x: int = 42;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => {
                assert_eq!(l.name, "x");
                assert!(l.type_annotation.is_some());
                assert_eq!(l.value, Expr::IntLiteral(42));
            }
            _ => panic!("expected let statement"),
        }
    }

    #[test]
    fn let_without_type() {
        let prog = parse(r#"let msg = "hello";"#);
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => {
                assert_eq!(l.name, "msg");
                assert!(l.type_annotation.is_none());
                assert_eq!(l.value, Expr::StringLiteral("hello".into()));
            }
            _ => panic!("expected let statement"),
        }
    }

    // --- Return ---

    #[test]
    fn return_with_value() {
        let prog = parse("fn f() { return 42; }");
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Return(r) => assert_eq!(r.value, Some(Expr::IntLiteral(42))),
                _ => panic!("expected return"),
            },
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn return_empty() {
        let prog = parse("fn f() { return; }");
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Return(r) => assert!(r.value.is_none()),
                _ => panic!("expected return"),
            },
            _ => panic!("expected function"),
        }
    }

    // --- Cb ---

    #[test]
    fn cb_statement() {
        let prog = parse("fn f() { cb 500; }");
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Cb(cb) => assert_eq!(cb.budget, 500),
                _ => panic!("expected cb statement"),
            },
            _ => panic!("expected function"),
        }
    }

    // --- Expressions ---

    #[test]
    fn arithmetic_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let prog = parse("let x = 1 + 2 * 3;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Binary(b) => {
                    assert_eq!(b.op, BinaryOp::Add);
                    assert_eq!(b.left, Expr::IntLiteral(1));
                    match &b.right {
                        Expr::Binary(r) => {
                            assert_eq!(r.op, BinaryOp::Mul);
                            assert_eq!(r.left, Expr::IntLiteral(2));
                            assert_eq!(r.right, Expr::IntLiteral(3));
                        }
                        _ => panic!("expected binary mul"),
                    }
                }
                _ => panic!("expected binary add"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn comparison() {
        let prog = parse("let x = a > 0;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Binary(b) => assert_eq!(b.op, BinaryOp::Gt),
                _ => panic!("expected binary"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn equality() {
        let prog = parse("let x = a == b;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Binary(b) => assert_eq!(b.op, BinaryOp::Eq),
                _ => panic!("expected binary"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn unary_neg() {
        let prog = parse("let x = -5;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Unary(u) => {
                    assert_eq!(u.op, UnaryOp::Neg);
                    assert_eq!(u.operand, Expr::IntLiteral(5));
                }
                _ => panic!("expected unary"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn unary_not() {
        let prog = parse("let x = !true;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Unary(u) => {
                    assert_eq!(u.op, UnaryOp::Not);
                    assert_eq!(u.operand, Expr::BoolLiteral(true));
                }
                _ => panic!("expected unary"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn parenthesized_expression() {
        // (1 + 2) * 3
        let prog = parse("let x = (1 + 2) * 3;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Binary(b) => {
                    assert_eq!(b.op, BinaryOp::Mul);
                    match &b.left {
                        Expr::Binary(inner) => {
                            assert_eq!(inner.op, BinaryOp::Add);
                        }
                        _ => panic!("expected inner binary"),
                    }
                }
                _ => panic!("expected binary"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn function_call() {
        let prog = parse(r#"print("hello", 42);"#);
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Expression(e)) => match &e.expr {
                Expr::Call(c) => {
                    assert_eq!(c.callee, "print");
                    assert_eq!(c.args.len(), 2);
                }
                _ => panic!("expected call"),
            },
            _ => panic!("expected expr statement"),
        }
    }

    #[test]
    fn field_access() {
        let prog = parse("let x = result.confidence;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::FieldAccess(fa) => {
                    assert_eq!(fa.field, "confidence");
                    assert_eq!(fa.object, Expr::Identifier("result".into()));
                }
                _ => panic!("expected field access"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn chained_field_access() {
        let prog = parse("let x = a.b.c;");
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::FieldAccess(outer) => {
                    assert_eq!(outer.field, "c");
                    match &outer.object {
                        Expr::FieldAccess(inner) => {
                            assert_eq!(inner.field, "b");
                            assert_eq!(inner.object, Expr::Identifier("a".into()));
                        }
                        _ => panic!("expected inner field access"),
                    }
                }
                _ => panic!("expected field access"),
            },
            _ => panic!("expected let"),
        }
    }

    // --- If ---

    #[test]
    fn if_expression() {
        let prog = parse("fn f() { if x > 0 { return 1; } }");
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Expression(e) => match &e.expr {
                    Expr::If(i) => {
                        assert!(i.else_block.is_none());
                    }
                    _ => panic!("expected if"),
                },
                _ => panic!("expected expr stmt"),
            },
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn if_else() {
        let prog = parse("fn f() { if x > 0 { return 1; } else { return 0; } }");
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Expression(e) => match &e.expr {
                    Expr::If(i) => {
                        assert!(i.else_block.is_some());
                    }
                    _ => panic!("expected if"),
                },
                _ => panic!("expected expr stmt"),
            },
            _ => panic!("expected function"),
        }
    }

    // --- AI-native constructs ---

    #[test]
    fn prompt_expression() {
        let prog = parse(r#"let r = prompt("Classify", data) -> Category;"#);
        match &prog.declarations[0] {
            Declaration::Statement(Statement::Let(l)) => match &l.value {
                Expr::Prompt(p) => {
                    assert_eq!(p.instruction, "Classify");
                    assert_eq!(p.input, Expr::Identifier("data".into()));
                    assert_eq!(p.return_type, TypeAnnotation::Named("Category".into()));
                }
                _ => panic!("expected prompt"),
            },
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn validate_expression() {
        let prog = parse(
            r#"
            fn f() {
                validate result {
                    result.confidence > 0.8,
                    result.label != "unknown"
                };
            }
        "#,
        );
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Expression(e) => match &e.expr {
                    Expr::Validate(v) => {
                        assert_eq!(v.target, Expr::Identifier("result".into()));
                        assert_eq!(v.predicates.len(), 2);
                    }
                    _ => panic!("expected validate"),
                },
                _ => panic!("expected expr stmt"),
            },
            _ => panic!("expected function"),
        }
    }

    #[test]
    fn explore_expression() {
        let prog = parse(
            r#"
            fn f() {
                explore "experiment-1" {
                    let x = 42;
                };
            }
        "#,
        );
        match &prog.declarations[0] {
            Declaration::Function(f) => match &f.body.statements[0] {
                Statement::Expression(e) => match &e.expr {
                    Expr::Explore(ex) => {
                        assert_eq!(ex.name, "experiment-1");
                        assert_eq!(ex.body.statements.len(), 1);
                    }
                    _ => panic!("expected explore"),
                },
                _ => panic!("expected expr stmt"),
            },
            _ => panic!("expected function"),
        }
    }

    // --- Full program ---

    #[test]
    fn full_agent_program() {
        let prog = parse(
            r#"
            type Report {
                summary: string,
                score: float
            }

            fn helper(x: int) -> int {
                return x + 1;
            }

            agent analyzer(data: string) -> Report {
                cb 500;
                let result = prompt("Analyze this", data) -> Report;
                validate result {
                    result.score > 0.5
                };
                return result;
            }
        "#,
        );
        assert_eq!(prog.declarations.len(), 3);
        assert!(matches!(&prog.declarations[0], Declaration::Type(_)));
        assert!(matches!(&prog.declarations[1], Declaration::Function(_)));
        assert!(matches!(&prog.declarations[2], Declaration::Agent(_)));
    }

    // --- Error cases ---

    #[test]
    fn missing_semicolon() {
        let err = parse_err("let x = 42");
        assert!(err.message.contains("expected Semicolon"));
    }

    #[test]
    fn missing_closing_brace() {
        let err = parse_err("fn f() { return 1;");
        assert!(err.message.contains("expected"), "got: {}", err.message);
    }

    #[test]
    fn missing_paren_in_call() {
        let err = parse_err("print(42;");
        assert!(err.message.contains("expected"), "got: {}", err.message);
    }

    #[test]
    fn invalid_prompt_instruction() {
        let err = parse_err("let x = prompt(42, data) -> Type;");
        assert!(err.message.contains("string literal"));
    }

    #[test]
    fn invalid_explore_name() {
        let err = parse_err("fn f() { explore 42 { }; }");
        assert!(err.message.contains("string literal"));
    }

    #[test]
    fn error_has_position() {
        let err = parse_err("let x = ;");
        assert!(err.line > 0);
        assert!(err.column > 0);
    }

    // --- Multi-error recovery ---

    #[test]
    fn multi_error_collects_multiple() {
        let result = Parser::parse_source_multi("let x = ; let y = 42; let z = ;");
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors.len() >= 2,
            "expected at least 2 errors, got {}",
            errors.len()
        );
    }

    #[test]
    fn multi_error_valid_source() {
        let result = Parser::parse_source_multi("let x = 42;");
        assert!(result.is_ok());
    }

    // --- List literal parsing ---

    #[test]
    fn parse_empty_list() {
        let program = parse("[];");
        match &program.declarations[0] {
            Declaration::Statement(Statement::Expression(es)) => {
                assert!(matches!(es.expr, Expr::ListLiteral(ref items) if items.is_empty()));
            }
            _ => panic!("expected expression statement"),
        }
    }

    #[test]
    fn parse_list_with_items() {
        let program = parse("[1, 2, 3];");
        match &program.declarations[0] {
            Declaration::Statement(Statement::Expression(es)) => {
                if let Expr::ListLiteral(items) = &es.expr {
                    assert_eq!(items.len(), 3);
                } else {
                    panic!("expected list literal");
                }
            }
            _ => panic!("expected expression statement"),
        }
    }

    #[test]
    fn parse_import_bare() {
        let program = parse(r#"import "abc123";"#);
        assert_eq!(program.declarations.len(), 1);
        match &program.declarations[0] {
            Declaration::Import(imp) => {
                assert_eq!(imp.hash, "abc123");
                assert!(imp.alias.is_none());
                assert!(imp.names.is_none());
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn parse_import_with_alias() {
        let program = parse(r#"import "deadbeef" as utils;"#);
        match &program.declarations[0] {
            Declaration::Import(imp) => {
                assert_eq!(imp.hash, "deadbeef");
                assert_eq!(imp.alias.as_deref(), Some("utils"));
                assert!(imp.names.is_none());
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn parse_import_selective() {
        let program = parse(r#"import "hash123" { foo, bar };"#);
        match &program.declarations[0] {
            Declaration::Import(imp) => {
                assert_eq!(imp.hash, "hash123");
                assert!(imp.alias.is_none());
                assert_eq!(imp.names.as_ref().unwrap(), &["foo", "bar"]);
            }
            _ => panic!("expected import"),
        }
    }

    #[test]
    fn parse_import_before_function() {
        let program = parse(r#"import "lib"; fn main() -> int { return 1; }"#);
        assert_eq!(program.declarations.len(), 2);
        assert!(matches!(&program.declarations[0], Declaration::Import(_)));
        assert!(matches!(&program.declarations[1], Declaration::Function(_)));
    }

    #[test]
    fn parse_spawn_no_args() {
        let program = parse("let h = spawn worker();");
        if let Declaration::Statement(Statement::Let(l)) = &program.declarations[0] {
            match &l.value {
                Expr::Spawn(s) => {
                    assert_eq!(s.agent_name, "worker");
                    assert!(s.args.is_empty());
                }
                _ => panic!("expected spawn expr"),
            }
        } else {
            panic!("expected let statement");
        }
    }

    #[test]
    fn parse_spawn_with_args() {
        let program = parse(r#"let h = spawn scanner("url", 42);"#);
        if let Declaration::Statement(Statement::Let(l)) = &program.declarations[0] {
            match &l.value {
                Expr::Spawn(s) => {
                    assert_eq!(s.agent_name, "scanner");
                    assert_eq!(s.args.len(), 2);
                }
                _ => panic!("expected spawn expr"),
            }
        } else {
            panic!("expected let statement");
        }
    }
}
