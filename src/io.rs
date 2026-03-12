//! Capability-gated I/O operations for Agentis agents.
//!
//! All file operations are sandboxed to `.agentis/sandbox/`.
//! All network operations are restricted to a domain whitelist.
//! Designed as a standalone module so a future WASM host can reuse
//! the same sandboxing logic.

use std::path::{Path, PathBuf};

use crate::config::Config;

/// Errors from I/O operations (before they become EvalErrors).
#[derive(Debug)]
pub enum IoError {
    PathOutsideSandbox(String),
    DomainNotWhitelisted(String),
    FileError(String),
    NetworkError(String),
}

impl std::fmt::Display for IoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IoError::PathOutsideSandbox(p) => write!(f, "path outside sandbox: {p}"),
            IoError::DomainNotWhitelisted(d) => write!(f, "domain not whitelisted: {d}"),
            IoError::FileError(e) => write!(f, "file I/O error: {e}"),
            IoError::NetworkError(e) => write!(f, "network error: {e}"),
        }
    }
}

/// Sandbox context for file and network operations.
pub struct IoContext {
    sandbox_dir: PathBuf,
    domain_whitelist: Vec<String>,
}

impl IoContext {
    /// Create an I/O context from the agentis root directory and config.
    pub fn new(agentis_root: &Path, config: &Config) -> Self {
        let sandbox_dir = agentis_root.join("sandbox");
        let domain_whitelist = match config.get("io.allowed_domains") {
            Some(domains) => domains
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => Vec::new(),
        };
        IoContext {
            sandbox_dir,
            domain_whitelist,
        }
    }

    /// Ensure the sandbox directory exists.
    pub fn ensure_sandbox(&self) -> Result<(), IoError> {
        if !self.sandbox_dir.exists() {
            std::fs::create_dir_all(&self.sandbox_dir)
                .map_err(|e| IoError::FileError(format!("cannot create sandbox: {e}")))?;
        }
        Ok(())
    }

    /// Resolve and validate a path is within the sandbox.
    /// Uses canonicalization to prevent `../` traversal.
    fn resolve_sandbox_path(&self, path: &str) -> Result<PathBuf, IoError> {
        self.ensure_sandbox()?;

        let candidate = self.sandbox_dir.join(path);

        // Canonicalize the sandbox root
        let canon_sandbox = self
            .sandbox_dir
            .canonicalize()
            .map_err(|e| IoError::FileError(format!("cannot canonicalize sandbox: {e}")))?;

        // For reads, the file must exist to canonicalize.
        // For writes, we canonicalize the parent directory.
        if candidate.exists() {
            let canon_path = candidate
                .canonicalize()
                .map_err(|e| IoError::FileError(format!("cannot canonicalize path: {e}")))?;
            if !canon_path.starts_with(&canon_sandbox) {
                return Err(IoError::PathOutsideSandbox(path.to_string()));
            }
            Ok(canon_path)
        } else {
            // File doesn't exist yet (write case) — canonicalize parent
            let parent = candidate
                .parent()
                .ok_or_else(|| IoError::PathOutsideSandbox(path.to_string()))?;
            // Create parent dirs if needed (still within sandbox)
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| IoError::FileError(format!("cannot create directory: {e}")))?;
            }
            let canon_parent = parent
                .canonicalize()
                .map_err(|e| IoError::FileError(format!("cannot canonicalize parent: {e}")))?;
            if !canon_parent.starts_with(&canon_sandbox) {
                return Err(IoError::PathOutsideSandbox(path.to_string()));
            }
            // Return the intended path (parent is validated)
            let file_name = candidate
                .file_name()
                .ok_or_else(|| IoError::PathOutsideSandbox(path.to_string()))?;
            Ok(canon_parent.join(file_name))
        }
    }

    /// Check if a URL's domain is in the whitelist.
    fn check_domain(&self, url: &str) -> Result<(), IoError> {
        if self.domain_whitelist.is_empty() {
            return Err(IoError::DomainNotWhitelisted(
                "no domains whitelisted (set io.allowed_domains in .agentis/config)".into(),
            ));
        }

        // Extract domain from URL
        let domain = extract_domain(url)
            .ok_or_else(|| IoError::NetworkError(format!("cannot parse domain from URL: {url}")))?;

        if self.domain_whitelist.iter().any(|d| d == &domain) {
            Ok(())
        } else {
            Err(IoError::DomainNotWhitelisted(domain))
        }
    }

    /// Read a file from the sandbox. Returns its content as a string.
    pub fn file_read(&self, path: &str) -> Result<String, IoError> {
        let resolved = self.resolve_sandbox_path(path)?;
        std::fs::read_to_string(&resolved)
            .map_err(|e| IoError::FileError(format!("{}: {e}", resolved.display())))
    }

    /// Write content to a file in the sandbox.
    pub fn file_write(&self, path: &str, content: &str) -> Result<(), IoError> {
        let resolved = self.resolve_sandbox_path(path)?;
        std::fs::write(&resolved, content)
            .map_err(|e| IoError::FileError(format!("{}: {e}", resolved.display())))
    }

    /// HTTP GET request to a whitelisted domain.
    pub fn http_get(&self, url: &str) -> Result<String, IoError> {
        self.check_domain(url)?;
        let mut response = ureq::get(url)
            .call()
            .map_err(|e| IoError::NetworkError(format!("GET {url}: {e}")))?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|e| IoError::NetworkError(format!("reading response: {e}")))
    }

    /// HTTP POST request to a whitelisted domain.
    pub fn http_post(&self, url: &str, body: &str) -> Result<String, IoError> {
        self.check_domain(url)?;
        let mut response = ureq::post(url)
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| IoError::NetworkError(format!("POST {url}: {e}")))?;
        response
            .body_mut()
            .read_to_string()
            .map_err(|e| IoError::NetworkError(format!("reading response: {e}")))
    }
}

/// Extract domain from a URL string (minimal parser, no dependencies).
fn extract_domain(url: &str) -> Option<String> {
    // Strip scheme
    let after_scheme = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else {
        return None;
    };
    // Take until / or : (port)
    let domain = after_scheme.split(['/', ':']).next()?;
    if domain.is_empty() {
        None
    } else {
        Some(domain.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tempfile;
    use std::fs;

    fn temp_sandbox() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path().join(".agentis");
        fs::create_dir_all(agentis_root.join("sandbox")).unwrap();
        (dir, agentis_root)
    }

    fn ctx(agentis_root: &Path, domains: &str) -> IoContext {
        let config_str = if domains.is_empty() {
            String::new()
        } else {
            format!("io.allowed_domains = {domains}")
        };
        let config = Config::parse(&config_str);
        IoContext::new(agentis_root, &config)
    }

    // --- extract_domain tests ---

    #[test]
    fn domain_https() {
        assert_eq!(
            extract_domain("https://example.com/path"),
            Some("example.com".into())
        );
    }

    #[test]
    fn domain_http_with_port() {
        assert_eq!(
            extract_domain("http://localhost:8080/api"),
            Some("localhost".into())
        );
    }

    #[test]
    fn domain_no_scheme() {
        assert_eq!(extract_domain("example.com/path"), None);
    }

    // --- file sandbox tests ---

    #[test]
    fn file_write_and_read() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        io.file_write("hello.txt", "world").unwrap();
        let content = io.file_read("hello.txt").unwrap();
        assert_eq!(content, "world");
    }

    #[test]
    fn file_read_nonexistent() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        let err = io.file_read("nope.txt").unwrap_err();
        assert!(matches!(err, IoError::FileError(_)));
    }

    #[test]
    fn file_write_subdirectory() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        io.file_write("sub/dir/file.txt", "nested").unwrap();
        let content = io.file_read("sub/dir/file.txt").unwrap();
        assert_eq!(content, "nested");
    }

    #[test]
    fn path_traversal_blocked() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        let err = io.file_write("../../etc/passwd", "evil").unwrap_err();
        assert!(matches!(err, IoError::PathOutsideSandbox(_)));
    }

    #[test]
    fn path_traversal_dotdot_in_middle() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        // Write a legit file first so sub/ exists
        io.file_write("sub/legit.txt", "ok").unwrap();
        // Try to escape via sub/../../
        let err = io.file_write("sub/../../escape.txt", "evil").unwrap_err();
        assert!(matches!(err, IoError::PathOutsideSandbox(_)));
    }

    // --- domain whitelist tests ---

    #[test]
    fn domain_not_whitelisted() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "api.example.com");
        let err = io.check_domain("https://evil.com/steal").unwrap_err();
        assert!(matches!(err, IoError::DomainNotWhitelisted(_)));
    }

    #[test]
    fn domain_whitelisted() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "api.example.com, other.org");
        assert!(io.check_domain("https://api.example.com/v1/data").is_ok());
        assert!(io.check_domain("https://other.org/info").is_ok());
    }

    #[test]
    fn empty_whitelist_denies_all() {
        let (_dir, root) = temp_sandbox();
        let io = ctx(&root, "");
        let err = io.check_domain("https://anything.com").unwrap_err();
        assert!(matches!(err, IoError::DomainNotWhitelisted(_)));
    }
}
