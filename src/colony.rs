// Distributed Colony infrastructure for agent evolution (Phase 8).
//
// M31: Thread pool for parallel local arena evaluation.
// M32: Worker node — TCP server for remote evaluation.

use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::fitness::FitnessWeights;
use crate::network::{
    self, NetworkError,
    MSG_EVAL, MSG_RESULT, MSG_PING, MSG_PONG,
    MSG_AUTH, MSG_AUTH_OK, MSG_AUTH_FAIL,
};

// --- Constants ---

const MAX_SOURCE_SIZE: usize = 1_048_576; // 1 MB
const MAX_OUTPUT_SIZE: usize = 4096;      // 4 KB

// Result status codes
const STATUS_OK: u8 = 0x00;
const STATUS_ERROR: u8 = 0x01;
const STATUS_REJECTED: u8 = 0x02;

// --- Thread Pool (M31) ---

/// A thread pool for parallel arena evaluation.
///
/// Worker threads pull jobs from a shared channel. Dropping the pool
/// (or calling `join`) closes the channel and waits for all workers
/// to finish their current jobs.
pub struct ThreadPool {
    workers: Vec<std::thread::JoinHandle<()>>,
    sender: Option<mpsc::Sender<Job>>,
}

type Job = Box<dyn FnOnce() + Send + 'static>;

impl ThreadPool {
    /// Create a thread pool with `size` worker threads.
    ///
    /// # Panics
    /// Panics if `size` is 0.
    pub fn new(size: usize) -> Self {
        assert!(size > 0, "thread pool size must be > 0");

        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));

        let mut workers = Vec::with_capacity(size);
        for _ in 0..size {
            let rx = Arc::clone(&receiver);
            let handle = std::thread::spawn(move || {
                loop {
                    // Hold the lock only long enough to receive one job.
                    let job = {
                        let lock = rx.lock().unwrap();
                        lock.recv()
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => break, // channel closed — shut down
                    }
                }
            });
            workers.push(handle);
        }

        ThreadPool {
            workers,
            sender: Some(sender),
        }
    }

    /// Submit a job to the pool. The job will be executed by the
    /// next available worker thread.
    pub fn execute<F: FnOnce() + Send + 'static>(&self, f: F) {
        if let Some(ref sender) = self.sender {
            sender.send(Box::new(f)).expect("thread pool channel closed");
        }
    }

    /// Shut down the pool: drop the sender to signal workers,
    /// then wait for all workers to finish.
    pub fn join(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Drop sender to signal workers to exit
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Detect available parallelism (CPU cores), fallback to 4.
pub fn detect_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

// --- Protocol Types (M32) ---

/// An evaluation request (EVAL message payload).
#[derive(Debug, Clone)]
pub struct EvalRequest {
    pub request_id: u32,
    pub source: String,
    pub budget: u64,
    pub weights: FitnessWeights,
    pub filename: String,
    pub grant_pii: bool,
    pub timeout_ms: u64,
}

/// An evaluation result (RESULT message payload).
#[derive(Debug, Clone)]
pub struct EvalResult {
    pub request_id: u32,
    pub status: u8,
    pub score: f64,
    pub cb_eff: f64,
    pub val_rate: f64,
    pub exp_rate: f64,
    pub prompt_count: u32,
    pub output: String,
    pub error: String,
    pub eval_time_ms: u64,
}

impl EvalResult {
    /// Convert to an ArenaEntry for ranking.
    pub fn to_arena_entry(&self, file: &str) -> crate::arena::ArenaEntry {
        if self.status != STATUS_OK || !self.error.is_empty() {
            let mut entry = crate::arena::ArenaEntry::from_error(file, &self.error);
            entry.score = self.score;
            entry
        } else {
            crate::arena::ArenaEntry {
                file: file.to_string(),
                score: self.score,
                cb_eff: self.cb_eff,
                val_rate: self.val_rate,
                exp_rate: self.exp_rate,
                prompt_count: self.prompt_count as usize,
                error: None,
                rounds: 1,
            }
        }
    }
}

// --- Protocol Encoding/Decoding ---

/// Encode an EvalRequest into a MSG_EVAL payload.
pub fn encode_eval_request(req: &EvalRequest) -> Vec<u8> {
    let source_bytes = req.source.as_bytes();
    let weights_str = req.weights.to_string();
    let weights_bytes = weights_str.as_bytes();
    let filename_bytes = req.filename.as_bytes();

    let mut buf = Vec::with_capacity(
        4 + 4 + source_bytes.len() + 8 + 4 + weights_bytes.len()
        + 4 + filename_bytes.len() + 1 + 8
    );

    buf.extend_from_slice(&req.request_id.to_le_bytes());
    buf.extend_from_slice(&(source_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(source_bytes);
    buf.extend_from_slice(&req.budget.to_le_bytes());
    buf.extend_from_slice(&(weights_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(weights_bytes);
    buf.extend_from_slice(&(filename_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(filename_bytes);
    buf.push(if req.grant_pii { 1 } else { 0 });
    buf.extend_from_slice(&req.timeout_ms.to_le_bytes());

    buf
}

/// Decode an EvalRequest from a MSG_EVAL payload.
pub fn decode_eval_request(payload: &[u8]) -> Result<EvalRequest, NetworkError> {
    let mut pos = 0;

    let request_id = read_u32(payload, &mut pos)?;
    let source_len = read_u32(payload, &mut pos)? as usize;
    if source_len > MAX_SOURCE_SIZE {
        return Err(NetworkError::Protocol(format!(
            "source too large: {} bytes (max {})", source_len, MAX_SOURCE_SIZE
        )));
    }
    let source = read_string(payload, &mut pos, source_len)?;
    let budget = read_u64(payload, &mut pos)?;
    let weights_len = read_u32(payload, &mut pos)? as usize;
    let weights_str = read_string(payload, &mut pos, weights_len)?;
    let filename_len = read_u32(payload, &mut pos)? as usize;
    let filename = read_string(payload, &mut pos, filename_len)?;
    let grant_pii = read_u8(payload, &mut pos)? != 0;
    let timeout_ms = read_u64(payload, &mut pos)?;

    let weights = FitnessWeights::parse(&weights_str)
        .map_err(|e| NetworkError::Protocol(format!("invalid weights: {e}")))?;

    Ok(EvalRequest {
        request_id,
        source,
        budget,
        weights,
        filename,
        grant_pii,
        timeout_ms,
    })
}

/// Encode an EvalResult into a MSG_RESULT payload.
pub fn encode_eval_result(res: &EvalResult) -> Vec<u8> {
    let output_bytes = truncate_bytes(res.output.as_bytes(), MAX_OUTPUT_SIZE);
    let error_bytes = truncate_bytes(res.error.as_bytes(), MAX_OUTPUT_SIZE);

    let mut buf = Vec::with_capacity(
        4 + 1 + 5 * 8 + 4 + 4 + output_bytes.len() + 4 + error_bytes.len() + 8
    );

    buf.extend_from_slice(&res.request_id.to_le_bytes());
    buf.push(res.status);
    buf.extend_from_slice(&res.score.to_le_bytes());
    buf.extend_from_slice(&res.cb_eff.to_le_bytes());
    buf.extend_from_slice(&res.val_rate.to_le_bytes());
    buf.extend_from_slice(&res.exp_rate.to_le_bytes());
    buf.extend_from_slice(&res.prompt_count.to_le_bytes());
    buf.extend_from_slice(&(output_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(output_bytes);
    buf.extend_from_slice(&(error_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(error_bytes);
    buf.extend_from_slice(&res.eval_time_ms.to_le_bytes());

    buf
}

/// Decode an EvalResult from a MSG_RESULT payload.
pub fn decode_eval_result(payload: &[u8]) -> Result<EvalResult, NetworkError> {
    let mut pos = 0;

    let request_id = read_u32(payload, &mut pos)?;
    let status = read_u8(payload, &mut pos)?;
    let score = read_f64(payload, &mut pos)?;
    let cb_eff = read_f64(payload, &mut pos)?;
    let val_rate = read_f64(payload, &mut pos)?;
    let exp_rate = read_f64(payload, &mut pos)?;
    let prompt_count = read_u32(payload, &mut pos)?;
    let output_len = read_u32(payload, &mut pos)? as usize;
    let output = read_string(payload, &mut pos, output_len)?;
    let error_len = read_u32(payload, &mut pos)? as usize;
    let error = read_string(payload, &mut pos, error_len)?;
    let eval_time_ms = read_u64(payload, &mut pos)?;

    Ok(EvalResult {
        request_id,
        status,
        score,
        cb_eff,
        val_rate,
        exp_rate,
        prompt_count,
        output,
        error,
        eval_time_ms,
    })
}

/// Encode a PING payload.
pub fn encode_ping() -> Vec<u8> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    ts.to_le_bytes().to_vec()
}

/// Decode a PING payload -> timestamp_ms.
pub fn decode_ping(payload: &[u8]) -> Result<u64, NetworkError> {
    let mut pos = 0;
    read_u64(payload, &mut pos)
}

/// Encode a PONG payload.
pub fn encode_pong(echo_ts: u64, stats: &WorkerStats, backend_name: &str) -> Vec<u8> {
    let backend_bytes = backend_name.as_bytes();
    let mut buf = Vec::with_capacity(8 + 4 + 4 + 8 + 1 + 4 + backend_bytes.len());

    buf.extend_from_slice(&echo_ts.to_le_bytes());
    buf.extend_from_slice(&stats.evals_completed.load(Ordering::Relaxed).to_le_bytes());
    buf.extend_from_slice(&stats.evals_failed.load(Ordering::Relaxed).to_le_bytes());
    buf.extend_from_slice(&stats.avg_eval_ms().to_le_bytes());
    buf.push(if stats.is_busy() { 1 } else { 0 });
    buf.extend_from_slice(&(backend_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(backend_bytes);

    buf
}

/// Decode a PONG payload.
pub fn decode_pong(payload: &[u8]) -> Result<PongData, NetworkError> {
    let mut pos = 0;
    let echo_ts = read_u64(payload, &mut pos)?;
    let evals_completed = read_u32(payload, &mut pos)?;
    let evals_failed = read_u32(payload, &mut pos)?;
    let avg_eval_ms = read_u64(payload, &mut pos)?;
    let busy = read_u8(payload, &mut pos)? != 0;
    let backend_len = read_u32(payload, &mut pos)? as usize;
    let backend = read_string(payload, &mut pos, backend_len)?;
    Ok(PongData { echo_ts, evals_completed, evals_failed, avg_eval_ms, busy, backend })
}

/// Decoded PONG data.
#[derive(Debug)]
pub struct PongData {
    pub echo_ts: u64,
    pub evals_completed: u32,
    pub evals_failed: u32,
    pub avg_eval_ms: u64,
    pub busy: bool,
    pub backend: String,
}

// --- Auth ---

/// Compute SHA-256 of secret bytes for auth handshake.
pub fn hash_secret(secret: &str) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Client-side auth handshake. Sends AUTH, expects AUTH_OK/AUTH_FAIL.
pub fn client_auth(stream: &mut TcpStream, secret: Option<&str>) -> Result<(), NetworkError> {
    if let Some(secret) = secret {
        let hash = hash_secret(secret);
        network::write_msg(stream, MSG_AUTH, &hash)?;

        let (msg_type, _) = network::read_msg(stream)?;
        match msg_type {
            MSG_AUTH_OK => Ok(()),
            MSG_AUTH_FAIL => Err(NetworkError::Protocol("authentication failed".to_string())),
            other => Err(NetworkError::Protocol(format!(
                "expected AUTH_OK/AUTH_FAIL, got 0x{other:02x}"
            ))),
        }
    } else {
        Ok(())
    }
}

/// Server-side auth handshake. Reads AUTH, sends AUTH_OK/AUTH_FAIL.
fn server_auth(stream: &mut TcpStream, secret: Option<&str>) -> Result<(), NetworkError> {
    if let Some(secret) = secret {
        let expected = hash_secret(secret);

        let (msg_type, payload) = network::read_msg(stream)?;
        if msg_type != MSG_AUTH {
            return Err(NetworkError::Protocol(format!(
                "expected AUTH (0x09), got 0x{msg_type:02x}"
            )));
        }
        if payload.len() != 32 || payload[..] != expected[..] {
            network::write_msg(stream, MSG_AUTH_FAIL, &[])?;
            return Err(NetworkError::Protocol("authentication failed".to_string()));
        }
        network::write_msg(stream, MSG_AUTH_OK, &[])?;
        Ok(())
    } else {
        Ok(())
    }
}

// --- Worker Stats ---

/// Atomic counters tracked by a worker node, shared across connection threads.
pub struct WorkerStats {
    pub evals_completed: AtomicU32,
    pub evals_failed: AtomicU32,
    pub total_eval_ms: AtomicU64,
    pub busy: AtomicU32,
}

impl WorkerStats {
    pub fn new() -> Self {
        Self {
            evals_completed: AtomicU32::new(0),
            evals_failed: AtomicU32::new(0),
            total_eval_ms: AtomicU64::new(0),
            busy: AtomicU32::new(0),
        }
    }

    pub fn avg_eval_ms(&self) -> u64 {
        let completed = self.evals_completed.load(Ordering::Relaxed) as u64;
        if completed == 0 { return 0; }
        self.total_eval_ms.load(Ordering::Relaxed) / completed
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Relaxed) > 0
    }
}

// --- Worker Server (M32) ---

/// Configuration for a worker node.
pub struct WorkerConfig {
    pub addr: String,
    pub secret: Option<String>,
    pub max_concurrent: usize,
    pub max_connections: usize,
}

/// Run a worker node that listens for EVAL/PING requests.
pub fn run_worker(config: WorkerConfig) -> Result<(), NetworkError> {
    let listener = TcpListener::bind(&config.addr)?;
    eprintln!("Worker listening on {} (max-concurrent: {}, max-connections: {})",
        config.addr, config.max_concurrent, config.max_connections);

    let stats = Arc::new(WorkerStats::new());

    // Load local LLM backend name for PONG responses
    let root = worker_root();
    let cfg = crate::config::Config::load(&root);
    let backend = crate::llm::create_backend(&cfg)
        .map_err(|e| NetworkError::Protocol(format!("LLM config error: {e}")))?;
    let backend_name = backend.name().to_string();
    drop(backend);

    let pool = Arc::new(ThreadPool::new(config.max_concurrent));
    let active_connections = Arc::new(AtomicU32::new(0));

    for stream_result in listener.incoming() {
        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[worker] accept error: {e}");
                continue;
            }
        };

        let current = active_connections.load(Ordering::Relaxed);
        if current >= config.max_connections as u32 {
            eprintln!("[worker] connection limit reached ({}/{}), rejecting",
                current, config.max_connections);
            continue;
        }

        // Auth handshake on the accept thread (before spawning handler)
        if let Err(e) = server_auth(&mut stream, config.secret.as_deref()) {
            let peer = stream.peer_addr()
                .map(|a| a.to_string()).unwrap_or_default();
            eprintln!("[worker] auth failed from {peer}: {e}");
            continue;
        }

        active_connections.fetch_add(1, Ordering::Relaxed);
        let stats = Arc::clone(&stats);
        let pool = Arc::clone(&pool);
        let conn_counter = Arc::clone(&active_connections);
        let backend_name = backend_name.clone();

        std::thread::spawn(move || {
            handle_worker_connection(&mut stream, &stats, &pool, &backend_name);
            conn_counter.fetch_sub(1, Ordering::Relaxed);
        });
    }

    Ok(())
}

/// Handle a single worker connection: read messages in a loop.
fn handle_worker_connection(
    stream: &mut TcpStream,
    stats: &WorkerStats,
    pool: &ThreadPool,
    backend_name: &str,
) {
    let peer = stream.peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    loop {
        let (msg_type, payload) = match network::read_msg(stream) {
            Ok(r) => r,
            Err(_) => break, // connection closed
        };

        match msg_type {
            MSG_EVAL => {
                let req = match decode_eval_request(&payload) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("[worker] bad EVAL from {peer}: {e}");
                        // Send error RESULT so coordinator doesn't hang
                        let err_result = EvalResult {
                            request_id: 0,
                            status: STATUS_REJECTED,
                            score: 0.0, cb_eff: 0.0, val_rate: 0.0, exp_rate: 0.0,
                            prompt_count: 0,
                            output: String::new(),
                            error: format!("{e}"),
                            eval_time_ms: 0,
                        };
                        let _ = network::write_msg(stream, MSG_RESULT,
                            &encode_eval_result(&err_result));
                        continue;
                    }
                };

                let req_id = req.request_id;
                let filename = req.filename.clone();
                let source_len = req.source.len();

                // Use the thread pool for concurrent evaluation
                let (result_tx, result_rx) = mpsc::channel::<EvalResult>();
                let stats_busy = Arc::new(AtomicU32::new(0));
                let busy_ref = Arc::clone(&stats_busy);

                pool.execute(move || {
                    busy_ref.fetch_add(1, Ordering::Relaxed);
                    let result = evaluate_locally(&req);
                    busy_ref.fetch_sub(1, Ordering::Relaxed);
                    let _ = result_tx.send(result);
                });

                stats.busy.fetch_add(1, Ordering::Relaxed);

                // Wait for result (blocking — this connection is handled
                // by its own thread, so blocking is fine)
                let result = match result_rx.recv() {
                    Ok(r) => r,
                    Err(_) => {
                        eprintln!("[worker] eval pool dropped for #{req_id}");
                        stats.busy.fetch_sub(1, Ordering::Relaxed);
                        break;
                    }
                };

                stats.busy.fetch_sub(1, Ordering::Relaxed);

                // Update stats
                if result.status == STATUS_OK {
                    stats.evals_completed.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats.evals_failed.fetch_add(1, Ordering::Relaxed);
                }
                stats.total_eval_ms.fetch_add(result.eval_time_ms, Ordering::Relaxed);

                // Log
                if result.error.is_empty() {
                    eprintln!("[worker] EVAL #{req_id} {filename} ({source_len}B) -> {:.3} ({}ms)",
                        result.score, result.eval_time_ms);
                } else {
                    eprintln!("[worker] EVAL #{req_id} {filename} ({source_len}B) -> error: {} ({}ms)",
                        truncate_str(&result.error, 60), result.eval_time_ms);
                }

                let result_payload = encode_eval_result(&result);
                if let Err(e) = network::write_msg(stream, MSG_RESULT, &result_payload) {
                    eprintln!("[worker] failed to send RESULT to {peer}: {e}");
                    break;
                }
            }
            MSG_PING => {
                let echo_ts = decode_ping(&payload).unwrap_or(0);
                let pong_payload = encode_pong(echo_ts, stats, backend_name);
                if let Err(e) = network::write_msg(stream, MSG_PONG, &pong_payload) {
                    eprintln!("[worker] failed to send PONG to {peer}: {e}");
                    break;
                }
            }
            other => {
                eprintln!("[worker] unexpected message type 0x{other:02x} from {peer}");
                break;
            }
        }
    }
}

/// Evaluate a program locally and return an EvalResult.
fn evaluate_locally(req: &EvalRequest) -> EvalResult {
    let start = Instant::now();

    let program = match crate::parser::Parser::parse_source(&req.source) {
        Ok(p) => p,
        Err(e) => {
            return EvalResult {
                request_id: req.request_id,
                status: STATUS_ERROR,
                score: 0.0, cb_eff: 0.0, val_rate: 0.0, exp_rate: 0.0,
                prompt_count: 0,
                output: String::new(),
                error: truncate_str(&format!("{e}"), MAX_OUTPUT_SIZE).to_string(),
                eval_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let root = worker_root();
    let cfg = crate::config::Config::load(&root);
    let llm_backend = match crate::llm::create_backend(&cfg) {
        Ok(b) => b,
        Err(e) => {
            return EvalResult {
                request_id: req.request_id,
                status: STATUS_ERROR,
                score: 0.0, cb_eff: 0.0, val_rate: 0.0, exp_rate: 0.0,
                prompt_count: 0,
                output: String::new(),
                error: truncate_str(&format!("{e}"), MAX_OUTPUT_SIZE).to_string(),
                eval_time_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let io_ctx = crate::io::IoContext::new(&root, &cfg);
    let tracer = crate::trace::Tracer::new(crate::trace::TraceLevel::Quiet);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;

    // Workers may not have an .agentis dir; create temp store if needed
    let (store, refs) = if root.join("objects").exists() {
        (crate::storage::ObjectStore::new(&root), crate::refs::Refs::new(&root))
    } else {
        let tmp = std::env::temp_dir().join(format!("agentis_worker_{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("objects"));
        let _ = std::fs::create_dir_all(tmp.join("refs"));
        (crate::storage::ObjectStore::new(&tmp), crate::refs::Refs::new(&tmp))
    };

    let _ = store.save(&program).ok();

    let mut evaluator = crate::evaluator::Evaluator::new(req.budget)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer);
    evaluator.grant_all();
    if req.grant_pii {
        evaluator.grant(crate::capabilities::CapKind::PiiTransmit);
    }

    let eval_error = evaluator.eval_program(&program).err();
    let report = evaluator.fitness_report();
    let output = evaluator.output().join("\n");
    let elapsed = start.elapsed().as_millis() as u64;

    match eval_error {
        None => EvalResult {
            request_id: req.request_id,
            status: STATUS_OK,
            score: report.score_with(&req.weights),
            cb_eff: report.cb_efficiency(),
            val_rate: report.validate_rate(),
            exp_rate: report.explore_rate(),
            prompt_count: report.prompt_count as u32,
            output: truncate_str(&output, MAX_OUTPUT_SIZE).to_string(),
            error: String::new(),
            eval_time_ms: elapsed,
        },
        Some(e) => EvalResult {
            request_id: req.request_id,
            status: STATUS_ERROR,
            score: 0.0,
            cb_eff: report.cb_efficiency(),
            val_rate: report.validate_rate(),
            exp_rate: report.explore_rate(),
            prompt_count: report.prompt_count as u32,
            output: truncate_str(&output, MAX_OUTPUT_SIZE).to_string(),
            error: truncate_str(&format!("{e}"), MAX_OUTPUT_SIZE).to_string(),
            eval_time_ms: elapsed,
        },
    }
}

/// Resolve the .agentis root path for workers.
fn worker_root() -> std::path::PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    cwd.join(".agentis")
}

// --- Binary helpers ---

fn read_u8(buf: &[u8], pos: &mut usize) -> Result<u8, NetworkError> {
    if *pos + 1 > buf.len() {
        return Err(NetworkError::Protocol("truncated payload (u8)".to_string()));
    }
    let val = buf[*pos];
    *pos += 1;
    Ok(val)
}

fn read_u32(buf: &[u8], pos: &mut usize) -> Result<u32, NetworkError> {
    if *pos + 4 > buf.len() {
        return Err(NetworkError::Protocol("truncated payload (u32)".to_string()));
    }
    let val = u32::from_le_bytes(buf[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

fn read_u64(buf: &[u8], pos: &mut usize) -> Result<u64, NetworkError> {
    if *pos + 8 > buf.len() {
        return Err(NetworkError::Protocol("truncated payload (u64)".to_string()));
    }
    let val = u64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_f64(buf: &[u8], pos: &mut usize) -> Result<f64, NetworkError> {
    if *pos + 8 > buf.len() {
        return Err(NetworkError::Protocol("truncated payload (f64)".to_string()));
    }
    let val = f64::from_le_bytes(buf[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_string(buf: &[u8], pos: &mut usize, len: usize) -> Result<String, NetworkError> {
    if *pos + len > buf.len() {
        return Err(NetworkError::Protocol("truncated payload (string)".to_string()));
    }
    let s = String::from_utf8(buf[*pos..*pos + len].to_vec())
        .map_err(|_| NetworkError::Protocol("invalid UTF-8".to_string()))?;
    *pos += len;
    Ok(s)
}

fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn truncate_bytes(b: &[u8], max: usize) -> &[u8] {
    if b.len() <= max { b } else { &b[..max] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn pool_runs_all_jobs() {
        let pool = ThreadPool::new(2);
        let (tx, rx) = mpsc::channel();

        for i in 0..8 {
            let tx = tx.clone();
            pool.execute(move || {
                tx.send(i).unwrap();
            });
        }
        drop(tx);
        pool.join();

        let mut results: Vec<i32> = rx.iter().collect();
        results.sort();
        assert_eq!(results, vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn pool_join_waits_for_completion() {
        let counter = Arc::new(AtomicUsize::new(0));
        let pool = ThreadPool::new(2);

        for _ in 0..10 {
            let c = Arc::clone(&counter);
            pool.execute(move || {
                // Simulate some work
                std::thread::sleep(std::time::Duration::from_millis(1));
                c.fetch_add(1, Ordering::Relaxed);
            });
        }

        pool.join();
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn pool_drop_waits_for_completion() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let pool = ThreadPool::new(2);
            for _ in 0..5 {
                let c = Arc::clone(&counter);
                pool.execute(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    c.fetch_add(1, Ordering::Relaxed);
                });
            }
            // pool dropped here
        }

        assert_eq!(counter.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn pool_single_thread() {
        let pool = ThreadPool::new(1);
        let (tx, rx) = mpsc::channel();

        for i in 0..4 {
            let tx = tx.clone();
            pool.execute(move || {
                tx.send(i).unwrap();
            });
        }
        drop(tx);
        pool.join();

        let mut results: Vec<i32> = rx.iter().collect();
        results.sort();
        assert_eq!(results, vec![0, 1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "thread pool size must be > 0")]
    fn pool_zero_size_panics() {
        let _ = ThreadPool::new(0);
    }

    #[test]
    fn pool_uses_multiple_threads() {
        // Submit jobs that block briefly so they must run on different threads
        let pool = ThreadPool::new(4);
        let thread_ids: Arc<Mutex<std::collections::HashSet<std::thread::ThreadId>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));

        // Use a barrier to force 4 jobs to run simultaneously
        let barrier = Arc::new(std::sync::Barrier::new(4));
        for _ in 0..4 {
            let ids = Arc::clone(&thread_ids);
            let b = Arc::clone(&barrier);
            pool.execute(move || {
                ids.lock().unwrap().insert(std::thread::current().id());
                b.wait(); // all 4 must be running at the same time
            });
        }

        pool.join();

        let ids = thread_ids.lock().unwrap();
        assert_eq!(ids.len(), 4, "expected 4 threads, got {}", ids.len());
    }

    #[test]
    fn detect_threads_returns_positive() {
        assert!(detect_threads() >= 1);
    }

    #[test]
    fn pool_heavy_load() {
        // Stress test: many small jobs
        let pool = ThreadPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..1000 {
            let c = Arc::clone(&counter);
            pool.execute(move || {
                c.fetch_add(1, Ordering::Relaxed);
            });
        }

        pool.join();
        assert_eq!(counter.load(Ordering::Relaxed), 1000);
    }

    // --- M32: Protocol encoding/decoding tests ---

    #[test]
    fn eval_request_roundtrip() {
        let req = EvalRequest {
            request_id: 42,
            source: "agent Foo { instruct \"hello\" }".to_string(),
            budget: 5000,
            weights: crate::fitness::FitnessWeights::default(),
            filename: "test.ag".to_string(),
            grant_pii: false,
            timeout_ms: 30_000,
        };

        let encoded = encode_eval_request(&req);
        let decoded = decode_eval_request(&encoded).unwrap();

        assert_eq!(decoded.request_id, 42);
        assert_eq!(decoded.source, req.source);
        assert_eq!(decoded.budget, 5000);
        assert_eq!(decoded.filename, "test.ag");
        assert!(!decoded.grant_pii);
        assert_eq!(decoded.timeout_ms, 30_000);
    }

    #[test]
    fn eval_request_with_pii_grant() {
        let req = EvalRequest {
            request_id: 1,
            source: "let x = 1;".to_string(),
            budget: 10_000,
            weights: crate::fitness::FitnessWeights::new(0.5, 0.3, 0.2),
            filename: "pii.ag".to_string(),
            grant_pii: true,
            timeout_ms: 60_000,
        };

        let encoded = encode_eval_request(&req);
        let decoded = decode_eval_request(&encoded).unwrap();

        assert!(decoded.grant_pii);
        assert_eq!(decoded.weights.w_cb, 0.5);
        assert_eq!(decoded.weights.w_val, 0.3);
        assert_eq!(decoded.weights.w_exp, 0.2);
    }

    #[test]
    fn eval_request_oversized_source_rejected() {
        // Build a payload with a source length field > MAX_SOURCE_SIZE
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // request_id
        buf.extend_from_slice(&(MAX_SOURCE_SIZE as u32 + 1).to_le_bytes()); // source_len (too big)
        // Don't need actual bytes — decode should bail on the size check

        let result = decode_eval_request(&buf);
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("source too large"), "got: {err}");
    }

    #[test]
    fn eval_result_roundtrip() {
        let res = EvalResult {
            request_id: 7,
            status: STATUS_OK,
            score: 0.85,
            cb_eff: 0.9,
            val_rate: 0.75,
            exp_rate: 1.0,
            prompt_count: 3,
            output: "hello world".to_string(),
            error: String::new(),
            eval_time_ms: 123,
        };

        let encoded = encode_eval_result(&res);
        let decoded = decode_eval_result(&encoded).unwrap();

        assert_eq!(decoded.request_id, 7);
        assert_eq!(decoded.status, STATUS_OK);
        assert!((decoded.score - 0.85).abs() < 1e-10);
        assert!((decoded.cb_eff - 0.9).abs() < 1e-10);
        assert!((decoded.val_rate - 0.75).abs() < 1e-10);
        assert!((decoded.exp_rate - 1.0).abs() < 1e-10);
        assert_eq!(decoded.prompt_count, 3);
        assert_eq!(decoded.output, "hello world");
        assert!(decoded.error.is_empty());
        assert_eq!(decoded.eval_time_ms, 123);
    }

    #[test]
    fn eval_result_with_error() {
        let res = EvalResult {
            request_id: 99,
            status: STATUS_ERROR,
            score: 0.0,
            cb_eff: 0.0,
            val_rate: 0.0,
            exp_rate: 0.0,
            prompt_count: 0,
            output: String::new(),
            error: "CognitiveOverload: budget exhausted".to_string(),
            eval_time_ms: 500,
        };

        let encoded = encode_eval_result(&res);
        let decoded = decode_eval_result(&encoded).unwrap();

        assert_eq!(decoded.status, STATUS_ERROR);
        assert_eq!(decoded.error, "CognitiveOverload: budget exhausted");
    }

    #[test]
    fn eval_result_truncates_long_output() {
        let long_output = "x".repeat(MAX_OUTPUT_SIZE + 1000);
        let res = EvalResult {
            request_id: 1,
            status: STATUS_OK,
            score: 1.0,
            cb_eff: 1.0,
            val_rate: 1.0,
            exp_rate: 1.0,
            prompt_count: 0,
            output: long_output,
            error: String::new(),
            eval_time_ms: 0,
        };

        let encoded = encode_eval_result(&res);
        let decoded = decode_eval_result(&encoded).unwrap();

        // Output should be truncated to MAX_OUTPUT_SIZE
        assert_eq!(decoded.output.len(), MAX_OUTPUT_SIZE);
    }

    #[test]
    fn ping_pong_roundtrip() {
        let ping_payload = encode_ping();
        let ts = decode_ping(&ping_payload).unwrap();
        assert!(ts > 0, "timestamp should be positive");

        let stats = WorkerStats::new();
        stats.evals_completed.store(10, Ordering::Relaxed);
        stats.evals_failed.store(2, Ordering::Relaxed);
        stats.total_eval_ms.store(5000, Ordering::Relaxed);
        stats.busy.store(1, Ordering::Relaxed);

        let pong_payload = encode_pong(ts, &stats, "mock");
        let pong = decode_pong(&pong_payload).unwrap();

        assert_eq!(pong.echo_ts, ts);
        assert_eq!(pong.evals_completed, 10);
        assert_eq!(pong.evals_failed, 2);
        assert_eq!(pong.avg_eval_ms, 500); // 5000 / 10
        assert!(pong.busy);
        assert_eq!(pong.backend, "mock");
    }

    #[test]
    fn worker_stats_defaults() {
        let stats = WorkerStats::new();
        assert_eq!(stats.evals_completed.load(Ordering::Relaxed), 0);
        assert_eq!(stats.evals_failed.load(Ordering::Relaxed), 0);
        assert_eq!(stats.avg_eval_ms(), 0);
        assert!(!stats.is_busy());
    }

    #[test]
    fn worker_stats_avg_eval() {
        let stats = WorkerStats::new();
        stats.evals_completed.store(4, Ordering::Relaxed);
        stats.total_eval_ms.store(400, Ordering::Relaxed);
        assert_eq!(stats.avg_eval_ms(), 100);
    }

    #[test]
    fn hash_secret_deterministic() {
        let h1 = hash_secret("my-secret-key");
        let h2 = hash_secret("my-secret-key");
        assert_eq!(h1, h2);

        let h3 = hash_secret("different-key");
        assert_ne!(h1, h3);
    }

    #[test]
    fn hash_secret_is_32_bytes() {
        let h = hash_secret("test");
        assert_eq!(h.len(), 32);
    }

    #[test]
    fn auth_handshake_success() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let secret = "test-secret";

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            server_auth(&mut stream, Some(secret)).unwrap();
        });

        let mut client = std::net::TcpStream::connect(&addr).unwrap();
        client_auth(&mut client, Some(secret)).unwrap();

        handle.join().unwrap();
    }

    #[test]
    fn auth_handshake_wrong_secret() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let result = server_auth(&mut stream, Some("correct-secret"));
            assert!(result.is_err());
        });

        let mut client = std::net::TcpStream::connect(&addr).unwrap();
        let result = client_auth(&mut client, Some("wrong-secret"));
        assert!(result.is_err());

        handle.join().unwrap();
    }

    #[test]
    fn auth_handshake_none_skips() {
        // No secret = no auth messages exchanged
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            server_auth(&mut stream, None).unwrap();
        });

        let mut client = std::net::TcpStream::connect(&addr).unwrap();
        client_auth(&mut client, None).unwrap();

        handle.join().unwrap();
    }

    #[test]
    fn eval_result_to_arena_entry_ok() {
        let res = EvalResult {
            request_id: 1,
            status: STATUS_OK,
            score: 0.75,
            cb_eff: 0.8,
            val_rate: 0.9,
            exp_rate: 1.0,
            prompt_count: 2,
            output: String::new(),
            error: String::new(),
            eval_time_ms: 100,
        };

        let entry = res.to_arena_entry("test.ag");
        assert_eq!(entry.file, "test.ag");
        assert!((entry.score - 0.75).abs() < 1e-10);
        assert_eq!(entry.prompt_count, 2);
        assert!(entry.error.is_none());
    }

    #[test]
    fn eval_result_to_arena_entry_error() {
        let res = EvalResult {
            request_id: 1,
            status: STATUS_ERROR,
            score: 0.0,
            cb_eff: 0.0,
            val_rate: 0.0,
            exp_rate: 0.0,
            prompt_count: 0,
            output: String::new(),
            error: "parse error".to_string(),
            eval_time_ms: 10,
        };

        let entry = res.to_arena_entry("bad.ag");
        assert!(entry.error.is_some());
        assert!(entry.error.unwrap().contains("parse error"));
    }

    #[test]
    fn worker_ping_pong_over_tcp() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Server: read PING, send PONG
            let (msg_type, payload) = crate::network::read_msg(&mut stream).unwrap();
            assert_eq!(msg_type, crate::network::MSG_PING);
            let echo_ts = decode_ping(&payload).unwrap();
            let stats = WorkerStats::new();
            stats.evals_completed.store(5, Ordering::Relaxed);
            let pong_payload = encode_pong(echo_ts, &stats, "test-backend");
            crate::network::write_msg(&mut stream, crate::network::MSG_PONG, &pong_payload).unwrap();
        });

        let mut client = std::net::TcpStream::connect(&addr).unwrap();
        // Client: send PING
        let ping = encode_ping();
        let sent_ts = decode_ping(&ping).unwrap();
        crate::network::write_msg(&mut client, crate::network::MSG_PING, &ping).unwrap();

        // Client: read PONG
        let (msg_type, payload) = crate::network::read_msg(&mut client).unwrap();
        assert_eq!(msg_type, crate::network::MSG_PONG);
        let pong = decode_pong(&payload).unwrap();
        assert_eq!(pong.echo_ts, sent_ts);
        assert_eq!(pong.evals_completed, 5);
        assert_eq!(pong.backend, "test-backend");

        handle.join().unwrap();
    }

    #[test]
    fn eval_request_empty_source() {
        let req = EvalRequest {
            request_id: 0,
            source: String::new(),
            budget: 100,
            weights: crate::fitness::FitnessWeights::default(),
            filename: String::new(),
            grant_pii: false,
            timeout_ms: 0,
        };

        let encoded = encode_eval_request(&req);
        let decoded = decode_eval_request(&encoded).unwrap();
        assert!(decoded.source.is_empty());
        assert!(decoded.filename.is_empty());
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 5), "hello");
        assert_eq!(truncate_str("hello", 3), "hel");
    }

    #[test]
    fn truncate_bytes_short() {
        assert_eq!(truncate_bytes(b"hello", 10), b"hello");
        assert_eq!(truncate_bytes(b"hello", 3), b"hel");
    }
}
