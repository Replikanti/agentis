use crate::compiler::CompileError;
use crate::evaluator::EvalError;
use crate::lexer::LexerError;
use crate::parser::ParseError;
use crate::refs::RefsError;
use crate::snapshot::SnapshotError;
use crate::storage::StorageError;

/// Unified error type for the Agentis system.
#[derive(Debug)]
pub enum AgentisError {
    Lexer(LexerError),
    Parse(ParseError),
    Eval(EvalError),
    Compile(CompileError),
    Snapshot(SnapshotError),
    Storage(StorageError),
    Refs(RefsError),
    Io(std::io::Error),
    General(String),
}

impl std::fmt::Display for AgentisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentisError::Lexer(e) => write!(f, "lexer error: {e}"),
            AgentisError::Parse(e) => write!(f, "parse error: {e}"),
            AgentisError::Eval(e) => write!(f, "runtime error: {e}"),
            AgentisError::Compile(e) => write!(f, "compile error: {e}"),
            AgentisError::Snapshot(e) => write!(f, "snapshot error: {e}"),
            AgentisError::Storage(e) => write!(f, "storage error: {e}"),
            AgentisError::Refs(e) => write!(f, "refs error: {e}"),
            AgentisError::Io(e) => write!(f, "I/O error: {e}"),
            AgentisError::General(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<LexerError> for AgentisError {
    fn from(e: LexerError) -> Self {
        AgentisError::Lexer(e)
    }
}

impl From<ParseError> for AgentisError {
    fn from(e: ParseError) -> Self {
        AgentisError::Parse(e)
    }
}

impl From<EvalError> for AgentisError {
    fn from(e: EvalError) -> Self {
        AgentisError::Eval(e)
    }
}

impl From<CompileError> for AgentisError {
    fn from(e: CompileError) -> Self {
        AgentisError::Compile(e)
    }
}

impl From<SnapshotError> for AgentisError {
    fn from(e: SnapshotError) -> Self {
        AgentisError::Snapshot(e)
    }
}

impl From<StorageError> for AgentisError {
    fn from(e: StorageError) -> Self {
        AgentisError::Storage(e)
    }
}

impl From<RefsError> for AgentisError {
    fn from(e: RefsError) -> Self {
        AgentisError::Refs(e)
    }
}

impl From<std::io::Error> for AgentisError {
    fn from(e: std::io::Error) -> Self {
        AgentisError::Io(e)
    }
}

impl From<String> for AgentisError {
    fn from(msg: String) -> Self {
        AgentisError::General(msg)
    }
}
