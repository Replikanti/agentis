use std::collections::HashMap;

use crate::evaluator::Value;
use crate::storage::{Hash, ObjectStore, StorageError};

// --- Snapshot Error ---

#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotError {
    Storage(String),
    DeserializeError(String),
    InvalidFormat(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Storage(msg) => write!(f, "snapshot storage error: {msg}"),
            SnapshotError::DeserializeError(msg) => {
                write!(f, "snapshot deserialization error: {msg}")
            }
            SnapshotError::InvalidFormat(msg) => write!(f, "invalid snapshot format: {msg}"),
        }
    }
}

impl From<StorageError> for SnapshotError {
    fn from(e: StorageError) -> Self {
        SnapshotError::Storage(format!("{e}"))
    }
}

// --- Snapshot Tags ---

const TAG_SNAPSHOT: u8 = 0xA0;
const TAG_VALUE_INT: u8 = 0xB0;
const TAG_VALUE_FLOAT: u8 = 0xB1;
const TAG_VALUE_STRING: u8 = 0xB2;
const TAG_VALUE_BOOL: u8 = 0xB3;
const TAG_VALUE_STRUCT: u8 = 0xB4;
const TAG_VALUE_VOID: u8 = 0xB5;
const TAG_VALUE_LIST: u8 = 0xB6;
const TAG_VALUE_MAP: u8 = 0xB7;
const TAG_SCOPE: u8 = 0xC0;

// --- Memory Snapshot ---

#[derive(Debug, Clone, PartialEq)]
pub struct MemorySnapshot {
    pub scopes: Vec<HashMap<String, Value>>,
    pub budget_remaining: u64,
    pub output: Vec<String>,
}

impl MemorySnapshot {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(TAG_SNAPSHOT);

        // Budget
        buf.extend_from_slice(&self.budget_remaining.to_le_bytes());

        // Number of output lines
        write_u32(&mut buf, self.output.len() as u32);
        for line in &self.output {
            write_string(&mut buf, line);
        }

        // Number of scopes
        write_u32(&mut buf, self.scopes.len() as u32);
        for scope in &self.scopes {
            buf.push(TAG_SCOPE);
            write_u32(&mut buf, scope.len() as u32);
            // Sort keys for deterministic serialization
            let mut entries: Vec<_> = scope.iter().collect();
            entries.sort_by_key(|(k, _)| k.clone());
            for (name, value) in entries {
                write_string(&mut buf, name);
                write_value(&mut buf, value);
            }
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, SnapshotError> {
        let mut pos = 0;

        if data.is_empty() || data[pos] != TAG_SNAPSHOT {
            return Err(SnapshotError::InvalidFormat(
                "missing snapshot tag".to_string(),
            ));
        }
        pos += 1;

        // Budget
        if pos + 8 > data.len() {
            return Err(SnapshotError::InvalidFormat("truncated budget".to_string()));
        }
        let budget_remaining = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
        pos += 8;

        // Output
        let output_count = read_u32(data, &mut pos)?;
        let mut output = Vec::with_capacity(output_count as usize);
        for _ in 0..output_count {
            output.push(read_string(data, &mut pos)?);
        }

        // Scopes
        let scope_count = read_u32(data, &mut pos)?;
        let mut scopes = Vec::with_capacity(scope_count as usize);
        for _ in 0..scope_count {
            if pos >= data.len() || data[pos] != TAG_SCOPE {
                return Err(SnapshotError::InvalidFormat(
                    "missing scope tag".to_string(),
                ));
            }
            pos += 1;

            let entry_count = read_u32(data, &mut pos)?;
            let mut scope = HashMap::new();
            for _ in 0..entry_count {
                let name = read_string(data, &mut pos)?;
                let value = read_value(data, &mut pos)?;
                scope.insert(name, value);
            }
            scopes.push(scope);
        }

        Ok(MemorySnapshot {
            scopes,
            budget_remaining,
            output,
        })
    }
}

// --- Snapshot Manager ---

pub struct SnapshotManager<'a> {
    store: &'a ObjectStore,
    history: Vec<Hash>,
}

impl<'a> SnapshotManager<'a> {
    pub fn new(store: &'a ObjectStore) -> Self {
        Self {
            store,
            history: Vec::new(),
        }
    }

    pub fn save(&mut self, snapshot: &MemorySnapshot) -> Result<Hash, SnapshotError> {
        let bytes = snapshot.to_bytes();
        let hash = self.store.save_raw(&bytes)?;
        self.history.push(hash.clone());
        Ok(hash)
    }

    pub fn load(&self, hash: &str) -> Result<MemorySnapshot, SnapshotError> {
        let bytes = self.store.load_raw(hash)?;
        MemorySnapshot::from_bytes(&bytes)
    }

    pub fn latest(&self) -> Option<&Hash> {
        self.history.last()
    }

    pub fn history(&self) -> &[Hash] {
        &self.history
    }

    pub fn rollback_to(&self, hash: &str) -> Result<MemorySnapshot, SnapshotError> {
        self.load(hash)
    }
}

// --- Serialization helpers ---

fn write_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn write_value(buf: &mut Vec<u8>, val: &Value) {
    match val {
        Value::Int(n) => {
            buf.push(TAG_VALUE_INT);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        Value::Float(n) => {
            buf.push(TAG_VALUE_FLOAT);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        Value::String(s) => {
            buf.push(TAG_VALUE_STRING);
            write_string(buf, s);
        }
        Value::Bool(b) => {
            buf.push(TAG_VALUE_BOOL);
            buf.push(if *b { 1 } else { 0 });
        }
        Value::Struct(name, fields) => {
            buf.push(TAG_VALUE_STRUCT);
            write_string(buf, name);
            write_u32(buf, fields.len() as u32);
            let mut entries: Vec<_> = fields.iter().collect();
            entries.sort_by_key(|(k, _)| k.clone());
            for (k, v) in entries {
                write_string(buf, k);
                write_value(buf, v);
            }
        }
        Value::Void => {
            buf.push(TAG_VALUE_VOID);
        }
        Value::List(items) => {
            buf.push(TAG_VALUE_LIST);
            write_u32(buf, items.len() as u32);
            for item in items {
                write_value(buf, item);
            }
        }
        Value::Map(entries) => {
            buf.push(TAG_VALUE_MAP);
            write_u32(buf, entries.len() as u32);
            let mut sorted: Vec<_> = entries.iter().collect();
            sorted.sort_by(|(a, _), (b, _)| format!("{a}").cmp(&format!("{b}")));
            for (k, v) in sorted {
                write_value(buf, k);
                write_value(buf, v);
            }
        }
        Value::AgentHandle(_) => {
            // Agent handles are transient — snapshot as void
            buf.push(TAG_VALUE_VOID);
        }
    }
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32, SnapshotError> {
    if *pos + 4 > data.len() {
        return Err(SnapshotError::InvalidFormat("truncated u32".to_string()));
    }
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, SnapshotError> {
    let len = read_u32(data, pos)? as usize;
    if *pos + len > data.len() {
        return Err(SnapshotError::InvalidFormat(
            "truncated string".to_string(),
        ));
    }
    let s = String::from_utf8(data[*pos..*pos + len].to_vec())
        .map_err(|e| SnapshotError::DeserializeError(format!("invalid UTF-8: {e}")))?;
    *pos += len;
    Ok(s)
}

fn read_value(data: &[u8], pos: &mut usize) -> Result<Value, SnapshotError> {
    if *pos >= data.len() {
        return Err(SnapshotError::InvalidFormat(
            "truncated value tag".to_string(),
        ));
    }
    let tag = data[*pos];
    *pos += 1;

    match tag {
        TAG_VALUE_INT => {
            if *pos + 8 > data.len() {
                return Err(SnapshotError::InvalidFormat("truncated int".to_string()));
            }
            let n = i64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(Value::Int(n))
        }
        TAG_VALUE_FLOAT => {
            if *pos + 8 > data.len() {
                return Err(SnapshotError::InvalidFormat("truncated float".to_string()));
            }
            let n = f64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
            *pos += 8;
            Ok(Value::Float(n))
        }
        TAG_VALUE_STRING => {
            let s = read_string(data, pos)?;
            Ok(Value::String(s))
        }
        TAG_VALUE_BOOL => {
            if *pos >= data.len() {
                return Err(SnapshotError::InvalidFormat("truncated bool".to_string()));
            }
            let b = data[*pos] != 0;
            *pos += 1;
            Ok(Value::Bool(b))
        }
        TAG_VALUE_STRUCT => {
            let name = read_string(data, pos)?;
            let field_count = read_u32(data, pos)?;
            let mut fields = HashMap::new();
            for _ in 0..field_count {
                let k = read_string(data, pos)?;
                let v = read_value(data, pos)?;
                fields.insert(k, v);
            }
            Ok(Value::Struct(name, fields))
        }
        TAG_VALUE_VOID => Ok(Value::Void),
        TAG_VALUE_LIST => {
            let count = read_u32(data, pos)?;
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(read_value(data, pos)?);
            }
            Ok(Value::List(items))
        }
        TAG_VALUE_MAP => {
            let count = read_u32(data, pos)?;
            let mut entries = Vec::new();
            for _ in 0..count {
                let k = read_value(data, pos)?;
                let v = read_value(data, pos)?;
                entries.push((k, v));
            }
            Ok(Value::Map(entries))
        }
        _ => Err(SnapshotError::InvalidFormat(format!(
            "unknown value tag: 0x{tag:02x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tempfile;

    fn test_store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path()).unwrap();
        (store, dir)
    }

    // --- MemorySnapshot serialization ---

    #[test]
    fn empty_snapshot_roundtrip() {
        let snap = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 10000,
            output: vec![],
        };
        let bytes = snap.to_bytes();
        let restored = MemorySnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(snap, restored);
    }

    #[test]
    fn snapshot_with_values_roundtrip() {
        let mut scope = HashMap::new();
        scope.insert("x".to_string(), Value::Int(42));
        scope.insert("y".to_string(), Value::Float(3.14));
        scope.insert("name".to_string(), Value::String("hello".to_string()));
        scope.insert("flag".to_string(), Value::Bool(true));

        let snap = MemorySnapshot {
            scopes: vec![scope],
            budget_remaining: 5000,
            output: vec!["line1".to_string(), "line2".to_string()],
        };
        let bytes = snap.to_bytes();
        let restored = MemorySnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(snap, restored);
    }

    #[test]
    fn snapshot_with_struct_value() {
        let mut fields = HashMap::new();
        fields.insert("score".to_string(), Value::Float(0.95));
        fields.insert("label".to_string(), Value::String("good".to_string()));

        let mut scope = HashMap::new();
        scope.insert("result".to_string(), Value::Struct("Report".to_string(), fields));

        let snap = MemorySnapshot {
            scopes: vec![scope],
            budget_remaining: 100,
            output: vec![],
        };
        let bytes = snap.to_bytes();
        let restored = MemorySnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(snap, restored);
    }

    #[test]
    fn snapshot_with_void() {
        let mut scope = HashMap::new();
        scope.insert("v".to_string(), Value::Void);

        let snap = MemorySnapshot {
            scopes: vec![scope],
            budget_remaining: 0,
            output: vec![],
        };
        let bytes = snap.to_bytes();
        let restored = MemorySnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(snap, restored);
    }

    #[test]
    fn snapshot_multiple_scopes() {
        let mut s1 = HashMap::new();
        s1.insert("a".to_string(), Value::Int(1));
        let mut s2 = HashMap::new();
        s2.insert("b".to_string(), Value::Int(2));

        let snap = MemorySnapshot {
            scopes: vec![s1, s2],
            budget_remaining: 999,
            output: vec![],
        };
        let bytes = snap.to_bytes();
        let restored = MemorySnapshot::from_bytes(&bytes).unwrap();
        assert_eq!(snap, restored);
    }

    #[test]
    fn snapshot_deterministic_serialization() {
        let mut scope = HashMap::new();
        scope.insert("z".to_string(), Value::Int(3));
        scope.insert("a".to_string(), Value::Int(1));
        scope.insert("m".to_string(), Value::Int(2));

        let snap = MemorySnapshot {
            scopes: vec![scope],
            budget_remaining: 100,
            output: vec![],
        };
        let bytes1 = snap.to_bytes();
        let bytes2 = snap.to_bytes();
        assert_eq!(bytes1, bytes2, "serialization must be deterministic");
    }

    #[test]
    fn invalid_snapshot_tag() {
        let result = MemorySnapshot::from_bytes(&[0xFF]);
        assert!(matches!(result, Err(SnapshotError::InvalidFormat(_))));
    }

    #[test]
    fn truncated_snapshot() {
        let result = MemorySnapshot::from_bytes(&[TAG_SNAPSHOT]);
        assert!(matches!(result, Err(SnapshotError::InvalidFormat(_))));
    }

    #[test]
    fn empty_data() {
        let result = MemorySnapshot::from_bytes(&[]);
        assert!(matches!(result, Err(SnapshotError::InvalidFormat(_))));
    }

    // --- SnapshotManager ---

    #[test]
    fn save_and_load_snapshot() {
        let (store, _dir) = test_store();
        let mut mgr = SnapshotManager::new(&store);

        let mut scope = HashMap::new();
        scope.insert("x".to_string(), Value::Int(42));

        let snap = MemorySnapshot {
            scopes: vec![scope],
            budget_remaining: 9000,
            output: vec!["hello".to_string()],
        };

        let hash = mgr.save(&snap).unwrap();
        let loaded = mgr.load(&hash).unwrap();
        assert_eq!(snap, loaded);
    }

    #[test]
    fn snapshot_is_content_addressed() {
        let (store, _dir) = test_store();
        let mut mgr = SnapshotManager::new(&store);

        let snap = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 100,
            output: vec![],
        };

        let hash1 = mgr.save(&snap).unwrap();
        let hash2 = mgr.save(&snap).unwrap();
        assert_eq!(hash1, hash2, "same snapshot should produce same hash");
    }

    #[test]
    fn different_snapshots_different_hashes() {
        let (store, _dir) = test_store();
        let mut mgr = SnapshotManager::new(&store);

        let snap1 = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 100,
            output: vec![],
        };
        let snap2 = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 200,
            output: vec![],
        };

        let hash1 = mgr.save(&snap1).unwrap();
        let hash2 = mgr.save(&snap2).unwrap();
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn snapshot_history() {
        let (store, _dir) = test_store();
        let mut mgr = SnapshotManager::new(&store);

        let snap1 = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 100,
            output: vec![],
        };
        let snap2 = MemorySnapshot {
            scopes: vec![],
            budget_remaining: 200,
            output: vec![],
        };

        assert!(mgr.latest().is_none());
        assert!(mgr.history().is_empty());

        let h1 = mgr.save(&snap1).unwrap();
        assert_eq!(mgr.latest(), Some(&h1));
        assert_eq!(mgr.history().len(), 1);

        let h2 = mgr.save(&snap2).unwrap();
        assert_eq!(mgr.latest(), Some(&h2));
        assert_eq!(mgr.history().len(), 2);
    }

    #[test]
    fn rollback_to_previous_snapshot() {
        let (store, _dir) = test_store();
        let mut mgr = SnapshotManager::new(&store);

        let mut s1 = HashMap::new();
        s1.insert("x".to_string(), Value::Int(1));
        let snap1 = MemorySnapshot {
            scopes: vec![s1],
            budget_remaining: 1000,
            output: vec![],
        };

        let mut s2 = HashMap::new();
        s2.insert("x".to_string(), Value::Int(2));
        let snap2 = MemorySnapshot {
            scopes: vec![s2],
            budget_remaining: 900,
            output: vec!["modified".to_string()],
        };

        let h1 = mgr.save(&snap1).unwrap();
        let _h2 = mgr.save(&snap2).unwrap();

        let restored = mgr.rollback_to(&h1).unwrap();
        assert_eq!(restored, snap1);
    }

    #[test]
    fn display_snapshot_error() {
        let e = SnapshotError::Storage("disk full".to_string());
        assert_eq!(format!("{e}"), "snapshot storage error: disk full");
        let e = SnapshotError::InvalidFormat("bad tag".to_string());
        assert_eq!(format!("{e}"), "invalid snapshot format: bad tag");
    }
}
