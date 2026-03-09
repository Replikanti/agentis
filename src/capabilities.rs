use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

// --- Capability Kinds ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CapKind {
    Prompt,
    FileRead,
    FileWrite,
    NetConnect,
    NetListen,
    VcsRead,
    VcsWrite,
    Stdout,
}

impl CapKind {
    pub fn all() -> &'static [CapKind] {
        &[
            CapKind::Prompt,
            CapKind::FileRead,
            CapKind::FileWrite,
            CapKind::NetConnect,
            CapKind::NetListen,
            CapKind::VcsRead,
            CapKind::VcsWrite,
            CapKind::Stdout,
        ]
    }

    fn as_byte(self) -> u8 {
        match self {
            CapKind::Prompt => 0x01,
            CapKind::FileRead => 0x02,
            CapKind::FileWrite => 0x03,
            CapKind::NetConnect => 0x04,
            CapKind::NetListen => 0x05,
            CapKind::VcsRead => 0x06,
            CapKind::VcsWrite => 0x07,
            CapKind::Stdout => 0x08,
        }
    }
}

impl std::fmt::Display for CapKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapKind::Prompt => write!(f, "prompt"),
            CapKind::FileRead => write!(f, "file_read"),
            CapKind::FileWrite => write!(f, "file_write"),
            CapKind::NetConnect => write!(f, "net_connect"),
            CapKind::NetListen => write!(f, "net_listen"),
            CapKind::VcsRead => write!(f, "vcs_read"),
            CapKind::VcsWrite => write!(f, "vcs_write"),
            CapKind::Stdout => write!(f, "stdout"),
        }
    }
}

// --- Capability Handle ---

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CapHandle {
    kind: CapKind,
    token: [u8; 32],
}

// --- Capability Error ---

#[derive(Debug, Clone, PartialEq)]
pub enum CapError {
    MissingCapability(CapKind),
    RevokedCapability(CapKind),
    InvalidHandle,
}

impl std::fmt::Display for CapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapError::MissingCapability(kind) => {
                write!(f, "missing capability: {kind}")
            }
            CapError::RevokedCapability(kind) => {
                write!(f, "revoked capability: {kind}")
            }
            CapError::InvalidHandle => write!(f, "invalid capability handle"),
        }
    }
}

// --- Capability Registry ---

pub struct CapabilityRegistry {
    secret: [u8; 32],
    counter: u64,
    granted: HashMap<[u8; 32], CapKind>,
    revoked: HashSet<[u8; 32]>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            secret: generate_secret(),
            counter: 0,
            granted: HashMap::new(),
            revoked: HashSet::new(),
        }
    }

    pub fn grant(&mut self, kind: CapKind) -> CapHandle {
        let token = self.mint_token(kind);
        self.granted.insert(token, kind);
        CapHandle { kind, token }
    }

    pub fn check(&self, handle: &CapHandle, kind: CapKind) -> Result<(), CapError> {
        if handle.kind != kind {
            return Err(CapError::InvalidHandle);
        }
        if self.revoked.contains(&handle.token) {
            return Err(CapError::RevokedCapability(kind));
        }
        if !self.granted.contains_key(&handle.token) {
            return Err(CapError::InvalidHandle);
        }
        Ok(())
    }

    pub fn revoke(&mut self, handle: &CapHandle) {
        self.revoked.insert(handle.token);
    }

    fn mint_token(&mut self, kind: CapKind) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.secret);
        hasher.update([kind.as_byte()]);
        hasher.update(self.counter.to_le_bytes());
        self.counter += 1;
        let result = hasher.finalize();
        let mut token = [0u8; 32];
        token.copy_from_slice(&result);
        token
    }
}

fn generate_secret() -> [u8; 32] {
    // Try /dev/urandom first (available on Linux/macOS)
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut secret = [0u8; 32];
        if f.read_exact(&mut secret).is_ok() {
            return secret;
        }
    }

    // Fallback: mix multiple entropy sources through SHA-256
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher_state = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher_state);
    std::process::id().hash(&mut hasher_state);
    std::thread::current().id().hash(&mut hasher_state);
    let seed = hasher_state.finish();

    let mut sha = Sha256::new();
    sha.update(seed.to_le_bytes());
    sha.update(b"agentis-capability-registry");
    let result = sha.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&result);
    secret
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_returns_valid_handle() {
        let mut reg = CapabilityRegistry::new();
        let handle = reg.grant(CapKind::Prompt);
        assert_eq!(handle.kind, CapKind::Prompt);
    }

    #[test]
    fn check_valid_handle_succeeds() {
        let mut reg = CapabilityRegistry::new();
        let handle = reg.grant(CapKind::Prompt);
        assert!(reg.check(&handle, CapKind::Prompt).is_ok());
    }

    #[test]
    fn check_wrong_kind_fails() {
        let mut reg = CapabilityRegistry::new();
        let handle = reg.grant(CapKind::Prompt);
        assert!(matches!(
            reg.check(&handle, CapKind::Stdout),
            Err(CapError::InvalidHandle)
        ));
    }

    #[test]
    fn check_revoked_fails() {
        let mut reg = CapabilityRegistry::new();
        let handle = reg.grant(CapKind::Prompt);
        reg.revoke(&handle);
        assert!(matches!(
            reg.check(&handle, CapKind::Prompt),
            Err(CapError::RevokedCapability(CapKind::Prompt))
        ));
    }

    #[test]
    fn check_unregistered_handle_fails() {
        let reg = CapabilityRegistry::new();
        let fake = CapHandle {
            kind: CapKind::Prompt,
            token: [0xAA; 32],
        };
        assert!(matches!(
            reg.check(&fake, CapKind::Prompt),
            Err(CapError::InvalidHandle)
        ));
    }

    #[test]
    fn different_registries_different_tokens() {
        let mut reg1 = CapabilityRegistry::new();
        let mut reg2 = CapabilityRegistry::new();
        let h1 = reg1.grant(CapKind::Prompt);
        let h2 = reg2.grant(CapKind::Prompt);
        // Different secrets → different tokens (with overwhelming probability)
        assert_ne!(h1.token, h2.token);
    }

    #[test]
    fn handles_are_unforgeable() {
        let mut reg = CapabilityRegistry::new();
        let _real = reg.grant(CapKind::Prompt);
        // Try to forge by constructing a handle manually
        let forged = CapHandle {
            kind: CapKind::Prompt,
            token: [0x42; 32],
        };
        assert!(reg.check(&forged, CapKind::Prompt).is_err());
    }

    #[test]
    fn grant_multiple_same_kind_unique_tokens() {
        let mut reg = CapabilityRegistry::new();
        let h1 = reg.grant(CapKind::Prompt);
        let h2 = reg.grant(CapKind::Prompt);
        assert_ne!(h1.token, h2.token);
        assert!(reg.check(&h1, CapKind::Prompt).is_ok());
        assert!(reg.check(&h2, CapKind::Prompt).is_ok());
    }

    #[test]
    fn display_cap_error() {
        let e = CapError::MissingCapability(CapKind::Prompt);
        assert_eq!(format!("{e}"), "missing capability: prompt");
        let e = CapError::RevokedCapability(CapKind::Stdout);
        assert_eq!(format!("{e}"), "revoked capability: stdout");
        let e = CapError::InvalidHandle;
        assert_eq!(format!("{e}"), "invalid capability handle");
    }

    #[test]
    fn display_cap_kind() {
        assert_eq!(format!("{}", CapKind::Prompt), "prompt");
        assert_eq!(format!("{}", CapKind::FileRead), "file_read");
        assert_eq!(format!("{}", CapKind::VcsWrite), "vcs_write");
    }

    #[test]
    fn revoke_nonexistent_is_noop() {
        let mut reg = CapabilityRegistry::new();
        let fake = CapHandle {
            kind: CapKind::Prompt,
            token: [0xFF; 32],
        };
        reg.revoke(&fake); // Should not panic
    }

    #[test]
    fn counter_increments() {
        let mut reg = CapabilityRegistry::new();
        let h1 = reg.grant(CapKind::Prompt);
        let h2 = reg.grant(CapKind::Prompt);
        let h3 = reg.grant(CapKind::Stdout);
        // All tokens must be unique
        assert_ne!(h1.token, h2.token);
        assert_ne!(h2.token, h3.token);
        assert_ne!(h1.token, h3.token);
    }
}
