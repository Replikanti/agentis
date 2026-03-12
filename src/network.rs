use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};

use crate::storage::{Hash, ObjectStore};

// --- Network Error ---

#[derive(Debug)]
pub enum NetworkError {
    Io(io::Error),
    Protocol(String),
    Storage(String),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::Io(e) => write!(f, "network I/O error: {e}"),
            NetworkError::Protocol(msg) => write!(f, "protocol error: {msg}"),
            NetworkError::Storage(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl From<io::Error> for NetworkError {
    fn from(e: io::Error) -> Self {
        NetworkError::Io(e)
    }
}

impl From<crate::storage::StorageError> for NetworkError {
    fn from(e: crate::storage::StorageError) -> Self {
        NetworkError::Storage(format!("{e}"))
    }
}

// --- Wire Protocol ---
//
// All messages are length-prefixed binary:
//   [u8 message_type] [u32 payload_length] [payload...]
//
// Message types:
//   0x01 HAVE  — sender's object hashes: [u32 count] [64-byte hex hash]*
//   0x02 WANT  — requested hashes:       [u32 count] [64-byte hex hash]*
//   0x03 DATA  — object data:            [u32 count] [64-byte hex hash + u32 len + bytes]*
//   0x04 DONE  — sync complete (empty payload)

const MSG_HAVE: u8 = 0x01;
const MSG_WANT: u8 = 0x02;
const MSG_DATA: u8 = 0x03;
const MSG_DONE: u8 = 0x04;

// Colony protocol extensions (Phase 8)
pub const MSG_EVAL: u8 = 0x05;
pub const MSG_RESULT: u8 = 0x06;
pub const MSG_PING: u8 = 0x07;
pub const MSG_PONG: u8 = 0x08;
pub const MSG_AUTH: u8 = 0x09;
pub const MSG_AUTH_OK: u8 = 0x0A;
pub const MSG_AUTH_FAIL: u8 = 0x0B;

const HASH_LEN: usize = 64; // SHA-256 hex string length

// --- Message encoding/decoding ---

pub fn write_msg(stream: &mut impl Write, msg_type: u8, payload: &[u8]) -> Result<(), NetworkError> {
    stream.write_all(&[msg_type])?;
    stream.write_all(&(payload.len() as u32).to_le_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;
    Ok(())
}

pub fn read_msg(stream: &mut impl Read) -> Result<(u8, Vec<u8>), NetworkError> {
    let mut header = [0u8; 5]; // 1 byte type + 4 bytes length
    stream.read_exact(&mut header)?;
    let msg_type = header[0];
    let len = u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize;

    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload)?;
    }
    Ok((msg_type, payload))
}

fn encode_hashes(hashes: &[Hash]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(hashes.len() as u32).to_le_bytes());
    for hash in hashes {
        buf.extend_from_slice(hash.as_bytes());
    }
    buf
}

fn decode_hashes(payload: &[u8]) -> Result<Vec<Hash>, NetworkError> {
    if payload.len() < 4 {
        return Err(NetworkError::Protocol("truncated hash list".to_string()));
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    let expected = 4 + count * HASH_LEN;
    if payload.len() < expected {
        return Err(NetworkError::Protocol(format!(
            "expected {expected} bytes for {count} hashes, got {}",
            payload.len()
        )));
    }

    let mut hashes = Vec::with_capacity(count);
    for i in 0..count {
        let start = 4 + i * HASH_LEN;
        let hash_bytes = &payload[start..start + HASH_LEN];
        let hash = String::from_utf8(hash_bytes.to_vec())
            .map_err(|_| NetworkError::Protocol("invalid hash encoding".to_string()))?;
        hashes.push(hash);
    }
    Ok(hashes)
}

fn encode_objects(objects: &[(Hash, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(objects.len() as u32).to_le_bytes());
    for (hash, data) in objects {
        buf.extend_from_slice(hash.as_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(data);
    }
    buf
}

fn decode_objects(payload: &[u8]) -> Result<Vec<(Hash, Vec<u8>)>, NetworkError> {
    if payload.len() < 4 {
        return Err(NetworkError::Protocol("truncated object list".to_string()));
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;

    let mut objects = Vec::with_capacity(count);
    let mut pos = 4;
    for _ in 0..count {
        if pos + HASH_LEN + 4 > payload.len() {
            return Err(NetworkError::Protocol("truncated object entry".to_string()));
        }
        let hash = String::from_utf8(payload[pos..pos + HASH_LEN].to_vec())
            .map_err(|_| NetworkError::Protocol("invalid hash encoding".to_string()))?;
        pos += HASH_LEN;

        let data_len = u32::from_le_bytes(payload[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;

        if pos + data_len > payload.len() {
            return Err(NetworkError::Protocol("truncated object data".to_string()));
        }
        let data = payload[pos..pos + data_len].to_vec();
        pos += data_len;

        objects.push((hash, data));
    }
    Ok(objects)
}

// --- Sync Operations ---

/// Sync as the initiating side (client). Connects to a peer and exchanges objects.
/// Returns the number of objects received.
pub fn sync_push_pull(store: &ObjectStore, addr: &str) -> Result<SyncResult, NetworkError> {
    let mut stream = TcpStream::connect(addr)?;
    sync_over_stream(store, &mut stream)
}

/// Listen for a single incoming sync connection, exchange objects, then return.
/// Returns the number of objects received.
pub fn sync_serve_once(store: &ObjectStore, addr: &str) -> Result<SyncResult, NetworkError> {
    let listener = TcpListener::bind(addr)?;
    let (mut stream, _peer_addr) = listener.accept()?;
    sync_over_stream(store, &mut stream)
}

/// The actual bidirectional sync protocol over an established stream.
///
/// Protocol flow (both sides run the same logic):
/// 1. Send HAVE (my hashes)
/// 2. Receive peer's HAVE
/// 3. Compute what I need (peer has, I don't) → send WANT
/// 4. Receive peer's WANT
/// 5. Send DATA for what peer wants
/// 6. Receive DATA for what I want
/// 7. Send DONE / receive DONE
pub fn sync_over_stream(
    store: &ObjectStore,
    stream: &mut TcpStream,
) -> Result<SyncResult, NetworkError> {
    let my_hashes = store.list_objects()?;

    // Step 1: Send my hashes
    let have_payload = encode_hashes(&my_hashes);
    write_msg(stream, MSG_HAVE, &have_payload)?;

    // Step 2: Receive peer's hashes
    let (msg_type, payload) = read_msg(stream)?;
    if msg_type != MSG_HAVE {
        return Err(NetworkError::Protocol(format!(
            "expected HAVE (0x01), got 0x{msg_type:02x}"
        )));
    }
    let peer_hashes = decode_hashes(&payload)?;

    // Step 3: Compute what I need and send WANT
    let my_set: std::collections::HashSet<&str> = my_hashes.iter().map(|h| h.as_str()).collect();
    let i_need: Vec<Hash> = peer_hashes
        .iter()
        .filter(|h| !my_set.contains(h.as_str()))
        .cloned()
        .collect();

    let want_payload = encode_hashes(&i_need);
    write_msg(stream, MSG_WANT, &want_payload)?;

    // Step 4: Receive peer's WANT
    let (msg_type, payload) = read_msg(stream)?;
    if msg_type != MSG_WANT {
        return Err(NetworkError::Protocol(format!(
            "expected WANT (0x02), got 0x{msg_type:02x}"
        )));
    }
    let peer_wants = decode_hashes(&payload)?;

    // Step 5: Send DATA for what peer wants
    let mut objects_to_send = Vec::new();
    for hash in &peer_wants {
        let data = store.load_raw(hash)?;
        objects_to_send.push((hash.clone(), data));
    }
    let data_payload = encode_objects(&objects_to_send);
    write_msg(stream, MSG_DATA, &data_payload)?;

    // Step 6: Receive DATA
    let (msg_type, payload) = read_msg(stream)?;
    if msg_type != MSG_DATA {
        return Err(NetworkError::Protocol(format!(
            "expected DATA (0x03), got 0x{msg_type:02x}"
        )));
    }
    let received_objects = decode_objects(&payload)?;
    let mut received_count = 0;
    for (_hash, data) in &received_objects {
        store.save_raw(data)?;
        received_count += 1;
    }

    // Step 7: Send DONE
    write_msg(stream, MSG_DONE, &[])?;

    // Receive DONE
    let (msg_type, _) = read_msg(stream)?;
    if msg_type != MSG_DONE {
        return Err(NetworkError::Protocol(format!(
            "expected DONE (0x04), got 0x{msg_type:02x}"
        )));
    }

    Ok(SyncResult {
        sent: objects_to_send.len(),
        received: received_count,
    })
}

#[derive(Debug, PartialEq)]
pub struct SyncResult {
    pub sent: usize,
    pub received: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::tempfile;
    use std::thread;

    fn test_store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path()).unwrap();
        (store, dir)
    }

    // --- Wire format tests ---

    #[test]
    fn encode_decode_hashes_roundtrip() {
        let hashes = vec![
            "a".repeat(64),
            "b".repeat(64),
        ];
        let encoded = encode_hashes(&hashes);
        let decoded = decode_hashes(&encoded).unwrap();
        assert_eq!(hashes, decoded);
    }

    #[test]
    fn encode_decode_empty_hashes() {
        let hashes: Vec<Hash> = vec![];
        let encoded = encode_hashes(&hashes);
        let decoded = decode_hashes(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn encode_decode_objects_roundtrip() {
        let objects = vec![
            ("a".repeat(64), b"hello".to_vec()),
            ("b".repeat(64), b"world".to_vec()),
        ];
        let encoded = encode_objects(&objects);
        let decoded = decode_objects(&encoded).unwrap();
        assert_eq!(objects, decoded);
    }

    #[test]
    fn encode_decode_empty_objects() {
        let objects: Vec<(Hash, Vec<u8>)> = vec![];
        let encoded = encode_objects(&objects);
        let decoded = decode_objects(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_hashes_truncated() {
        let result = decode_hashes(&[0x01]); // too short
        assert!(matches!(result, Err(NetworkError::Protocol(_))));
    }

    #[test]
    fn decode_objects_truncated() {
        let result = decode_objects(&[0x01]); // too short
        assert!(matches!(result, Err(NetworkError::Protocol(_))));
    }

    // --- Message framing tests ---

    #[test]
    fn write_read_msg_roundtrip() {
        let payload = b"test payload";
        let mut buf = Vec::new();
        write_msg(&mut buf, MSG_HAVE, payload).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let (msg_type, data) = read_msg(&mut cursor).unwrap();
        assert_eq!(msg_type, MSG_HAVE);
        assert_eq!(data, payload);
    }

    #[test]
    fn write_read_empty_msg() {
        let mut buf = Vec::new();
        write_msg(&mut buf, MSG_DONE, &[]).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let (msg_type, data) = read_msg(&mut cursor).unwrap();
        assert_eq!(msg_type, MSG_DONE);
        assert!(data.is_empty());
    }

    #[test]
    fn msg_types_are_distinct() {
        assert_ne!(MSG_HAVE, MSG_WANT);
        assert_ne!(MSG_WANT, MSG_DATA);
        assert_ne!(MSG_DATA, MSG_DONE);
    }

    // --- Full sync integration tests ---

    #[test]
    fn sync_empty_stores() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            sync_over_stream(&store_b, &mut stream).unwrap()
        });

        let mut stream = TcpStream::connect(&addr).unwrap();
        let result_a = sync_over_stream(&store_a, &mut stream).unwrap();
        let result_b = handle.join().unwrap();

        assert_eq!(result_a, SyncResult { sent: 0, received: 0 });
        assert_eq!(result_b, SyncResult { sent: 0, received: 0 });
    }

    #[test]
    fn sync_one_way_transfer() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        // Store A has objects, Store B is empty
        let h1 = store_a.save_raw(b"object-one").unwrap();
        let h2 = store_a.save_raw(b"object-two").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = sync_over_stream(&store_b, &mut stream).unwrap();
            (result, store_b)
        });

        let mut stream = TcpStream::connect(&addr).unwrap();
        let result_a = sync_over_stream(&store_a, &mut stream).unwrap();
        let (result_b, store_b) = handle.join().unwrap();

        // A sent 2 objects, received 0
        assert_eq!(result_a.sent, 2);
        assert_eq!(result_a.received, 0);

        // B received 2 objects, sent 0
        assert_eq!(result_b.received, 2);
        assert_eq!(result_b.sent, 0);

        // Verify B now has the objects
        assert!(store_b.load_raw(&h1).is_ok());
        assert!(store_b.load_raw(&h2).is_ok());
    }

    #[test]
    fn sync_bidirectional_transfer() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        // A has obj1, B has obj2
        let h1 = store_a.save_raw(b"only-in-a").unwrap();
        let h2 = store_b.save_raw(b"only-in-b").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = sync_over_stream(&store_b, &mut stream).unwrap();
            (result, store_b)
        });

        let mut stream = TcpStream::connect(&addr).unwrap();
        let result_a = sync_over_stream(&store_a, &mut stream).unwrap();
        let (result_b, store_b) = handle.join().unwrap();

        // A sent 1, received 1
        assert_eq!(result_a.sent, 1);
        assert_eq!(result_a.received, 1);

        // B sent 1, received 1
        assert_eq!(result_b.sent, 1);
        assert_eq!(result_b.received, 1);

        // Both stores now have both objects
        assert!(store_a.load_raw(&h2).is_ok());
        assert!(store_b.load_raw(&h1).is_ok());
    }

    #[test]
    fn sync_already_in_sync() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        // Both have the same object
        store_a.save_raw(b"shared-data").unwrap();
        store_b.save_raw(b"shared-data").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            sync_over_stream(&store_b, &mut stream).unwrap()
        });

        let mut stream = TcpStream::connect(&addr).unwrap();
        let result_a = sync_over_stream(&store_a, &mut stream).unwrap();
        let result_b = handle.join().unwrap();

        assert_eq!(result_a, SyncResult { sent: 0, received: 0 });
        assert_eq!(result_b, SyncResult { sent: 0, received: 0 });
    }

    #[test]
    fn sync_many_objects() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        // A has 10 unique objects
        let mut hashes = Vec::new();
        for i in 0..10 {
            let h = store_a.save_raw(format!("obj-{i}").as_bytes()).unwrap();
            hashes.push(h);
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = sync_over_stream(&store_b, &mut stream).unwrap();
            (result, store_b)
        });

        let mut stream = TcpStream::connect(&addr).unwrap();
        let result_a = sync_over_stream(&store_a, &mut stream).unwrap();
        let (result_b, store_b) = handle.join().unwrap();

        assert_eq!(result_a.sent, 10);
        assert_eq!(result_b.received, 10);

        for h in &hashes {
            assert!(store_b.load_raw(h).is_ok());
        }
    }

    #[test]
    fn sync_push_pull_convenience() {
        let (store_a, _dir_a) = test_store();
        let (store_b, _dir_b) = test_store();

        store_a.save_raw(b"from-client").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server_addr = addr.clone();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = sync_over_stream(&store_b, &mut stream).unwrap();
            (result, store_b)
        });

        let result = sync_push_pull(&store_a, &server_addr).unwrap();
        let (_, store_b) = handle.join().unwrap();

        assert_eq!(result.sent, 1);
        assert!(store_b.load_raw(&store_a.list_objects().unwrap()[0]).is_ok());
    }

    // --- Error handling ---

    #[test]
    fn display_network_error() {
        let e = NetworkError::Protocol("bad message".to_string());
        assert_eq!(format!("{e}"), "protocol error: bad message");
        let e = NetworkError::Storage("not found".to_string());
        assert_eq!(format!("{e}"), "storage error: not found");
    }

    #[test]
    fn sync_result_debug() {
        let r = SyncResult { sent: 3, received: 5 };
        let s = format!("{r:?}");
        assert!(s.contains("sent: 3"));
        assert!(s.contains("received: 5"));
    }
}
