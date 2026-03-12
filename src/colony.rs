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
    pub fn to_arena_entry(&self, file: &str, worker_addr: Option<&str>) -> crate::arena::ArenaEntry {
        if self.status != STATUS_OK || !self.error.is_empty() {
            let mut entry = crate::arena::ArenaEntry::from_error(file, &self.error);
            entry.score = self.score;
            entry.worker = worker_addr.map(|s| s.to_string());
            entry.eval_time_ms = Some(self.eval_time_ms);
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
                worker: worker_addr.map(|s| s.to_string()),
                eval_time_ms: Some(self.eval_time_ms),
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

// --- Colony Coordinator (M33) ---

/// Colony configuration for distributed arena evaluation.
pub struct ColonyConfig {
    pub workers: Vec<String>,
    pub secret: Option<String>,
    pub connect_timeout_ms: u64,
    pub eval_timeout_ms: u64,
}

/// Parse workers from a CLI value. If the value is a path to an existing file,
/// read it (one addr:port per line, blank lines and # comments ignored).
/// Otherwise split by comma.
pub fn parse_workers(value: &str) -> Vec<String> {
    let path = std::path::Path::new(value);
    if path.is_file() {
        if let Ok(contents) = std::fs::read_to_string(path) {
            return contents.lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect();
        }
    }
    value.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Evaluate a single variant on a remote worker. Returns an ArenaEntry.
/// On failure, logs a warning and returns None (caller should fall back to local).
pub fn evaluate_on_worker(
    worker_addr: &str,
    file: &str,
    source: &str,
    budget: u64,
    weights: &FitnessWeights,
    grant_pii: bool,
    secret: Option<&str>,
    connect_timeout_ms: u64,
    eval_timeout_ms: u64,
) -> Result<crate::arena::ArenaEntry, String> {
    // Connect with timeout
    let addr: std::net::SocketAddr = worker_addr.parse()
        .map_err(|e| format!("invalid address: {e}"))?;
    let connect_timeout = std::time::Duration::from_millis(connect_timeout_ms);
    let mut stream = TcpStream::connect_timeout(&addr, connect_timeout)
        .map_err(|e| categorize_connect_error(e))?;

    // Set read/write timeouts for eval
    let eval_timeout = std::time::Duration::from_millis(eval_timeout_ms);
    stream.set_read_timeout(Some(eval_timeout))
        .map_err(|e| format!("failed to set read timeout: {e}"))?;
    stream.set_write_timeout(Some(eval_timeout))
        .map_err(|e| format!("failed to set write timeout: {e}"))?;

    // Auth handshake
    client_auth(&mut stream, secret)
        .map_err(|e| categorize_auth_error(e))?;

    // Send EVAL
    let request_id = 1; // single eval per connection for now
    let req = EvalRequest {
        request_id,
        source: source.to_string(),
        budget,
        weights: weights.clone(),
        filename: file.to_string(),
        grant_pii,
        timeout_ms: eval_timeout_ms,
    };
    let payload = encode_eval_request(&req);
    network::write_msg(&mut stream, MSG_EVAL, &payload)
        .map_err(|e| categorize_protocol_error(e))?;

    // Read RESULT
    let (msg_type, result_payload) = network::read_msg(&mut stream)
        .map_err(|e| categorize_protocol_error(e))?;
    if msg_type != MSG_RESULT {
        return Err(format!("protocol error (expected RESULT 0x06, got 0x{msg_type:02x})"));
    }

    let result = decode_eval_result(&result_payload)
        .map_err(|e| format!("protocol error ({e})"))?;

    Ok(result.to_arena_entry(file, Some(worker_addr)))
}

/// Run arena evaluation across colony workers with local fallback.
/// Returns entries in the same order as `files`.
pub fn run_arena_colony(
    files: &[String],
    rounds: usize,
    colony: &ColonyConfig,
    root: &std::path::Path,
    grant_pii: bool,
    weights: &FitnessWeights,
    budget: u64,
) -> Vec<crate::arena::ArenaEntry> {
    let num_workers = colony.workers.len();
    eprintln!("Colony arena: {} variants, {} worker{}, {} round{} each",
        files.len(),
        num_workers,
        if num_workers == 1 { "" } else { "s" },
        rounds,
        if rounds == 1 { "" } else { "s" },
    );

    let mut all_entries = Vec::with_capacity(files.len());

    for (idx, file) in files.iter().enumerate() {
        let mut round_entries = Vec::new();

        for round in 0..rounds {
            // Round-robin worker assignment
            let worker_idx = (idx * rounds + round) % num_workers;
            let worker_addr = &colony.workers[worker_idx];

            // Read source
            let source = match std::fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) => {
                    round_entries.push(crate::arena::ArenaEntry::from_error(file, &format!("{e}")));
                    continue;
                }
            };

            if source.len() > MAX_SOURCE_SIZE {
                round_entries.push(crate::arena::ArenaEntry::from_error(
                    file, &format!("source too large ({} bytes, max {})", source.len(), MAX_SOURCE_SIZE)));
                continue;
            }

            match evaluate_on_worker(
                worker_addr, file, &source, budget, weights,
                grant_pii, colony.secret.as_deref(),
                colony.connect_timeout_ms, colony.eval_timeout_ms,
            ) {
                Ok(entry) => {
                    round_entries.push(entry);
                }
                Err(reason) => {
                    eprintln!("Warning: Worker {} {}, falling back to local",
                        worker_addr, reason);
                    // Fallback to local
                    let entry = evaluate_locally_for_arena(
                        file, &source, root, grant_pii, weights, budget);
                    round_entries.push(entry);
                }
            }
        }

        let entry = if round_entries.len() == 1 {
            round_entries.into_iter().next().unwrap()
        } else {
            crate::arena::ArenaEntry::average(&round_entries)
        };
        all_entries.push(entry);
    }

    all_entries
}

/// Local evaluation fallback for colony mode (returns ArenaEntry with worker="local").
fn evaluate_locally_for_arena(
    file: &str,
    source: &str,
    root: &std::path::Path,
    grant_pii: bool,
    weights: &FitnessWeights,
    budget: u64,
) -> crate::arena::ArenaEntry {
    let start = Instant::now();

    let program = match crate::parser::Parser::parse_source(source) {
        Ok(p) => p,
        Err(e) => {
            let mut entry = crate::arena::ArenaEntry::from_error(file, &format!("{e}"));
            entry.worker = Some("local".to_string());
            return entry;
        }
    };

    let cfg = crate::config::Config::load(root);
    let llm_backend = match crate::llm::create_backend(&cfg) {
        Ok(b) => b,
        Err(e) => {
            let mut entry = crate::arena::ArenaEntry::from_error(file, &format!("{e}"));
            entry.worker = Some("local".to_string());
            return entry;
        }
    };

    let io_ctx = crate::io::IoContext::new(root, &cfg);
    let tracer = crate::trace::Tracer::new(crate::trace::TraceLevel::Quiet);
    let max_agents = cfg.get_u64("max_concurrent_agents", 16) as u32;
    let store = crate::storage::ObjectStore::new(root);
    let refs = crate::refs::Refs::new(root);
    let _ = store.save(&program).ok();

    let mut evaluator = crate::evaluator::Evaluator::new(budget)
        .with_vcs(&store, &refs)
        .with_persistence(&store)
        .with_llm(llm_backend.as_ref())
        .with_io(&io_ctx)
        .with_max_agents(max_agents)
        .with_tracer(&tracer);
    evaluator.grant_all();
    if grant_pii {
        evaluator.grant(crate::capabilities::CapKind::PiiTransmit);
    }

    let elapsed = start.elapsed().as_millis() as u64;

    match evaluator.eval_program(&program) {
        Ok(_) => {
            let mut entry = crate::arena::ArenaEntry::from_report(
                file, &evaluator.fitness_report(), weights);
            entry.worker = Some("local".to_string());
            entry.eval_time_ms = Some(elapsed);
            entry
        }
        Err(e) => {
            let mut entry = crate::arena::ArenaEntry::from_error(file, &format!("{e}"));
            entry.worker = Some("local".to_string());
            entry.eval_time_ms = Some(elapsed);
            entry
        }
    }
}

/// Categorize TCP connect errors into user-friendly messages.
fn categorize_connect_error(e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => "unreachable (connection refused)".to_string(),
        std::io::ErrorKind::TimedOut => "connection timed out".to_string(),
        _ => format!("unreachable ({e})"),
    }
}

/// Categorize auth errors.
fn categorize_auth_error(e: NetworkError) -> String {
    match &e {
        NetworkError::Protocol(msg) if msg.contains("authentication failed") => {
            "auth failed".to_string()
        }
        _ => format!("{e}"),
    }
}

/// Categorize protocol errors.
fn categorize_protocol_error(e: NetworkError) -> String {
    match &e {
        NetworkError::Io(io_err) if io_err.kind() == std::io::ErrorKind::TimedOut => {
            "timed out".to_string()
        }
        NetworkError::Io(io_err) if io_err.kind() == std::io::ErrorKind::WouldBlock => {
            "timed out".to_string()
        }
        _ => format!("protocol error ({e})"),
    }
}

// --- Colony Observability (M34) ---

/// Result of pinging a single worker.
#[derive(Debug)]
pub struct WorkerStatus {
    pub addr: String,
    pub status: String,
    pub pong: Option<PongData>,
    pub latency_ms: Option<u64>,
}

/// Ping a single worker: connect, auth, send PING, read PONG.
/// Returns WorkerStatus with latency measured as PING/PONG roundtrip.
pub fn ping_worker(
    addr: &str,
    secret: Option<&str>,
    connect_timeout_ms: u64,
) -> WorkerStatus {
    let parsed: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(_) => return WorkerStatus {
            addr: addr.to_string(),
            status: "invalid-address".to_string(),
            pong: None,
            latency_ms: None,
        },
    };

    let timeout = std::time::Duration::from_millis(connect_timeout_ms);
    let mut stream = match TcpStream::connect_timeout(&parsed, timeout) {
        Ok(s) => s,
        Err(e) => {
            let reason = categorize_connect_error(e);
            return WorkerStatus {
                addr: addr.to_string(),
                status: format!("offline ({reason})"),
                pong: None,
                latency_ms: None,
            };
        }
    };

    // Set read/write timeout for the PING/PONG exchange
    let rw_timeout = std::time::Duration::from_secs(10);
    let _ = stream.set_read_timeout(Some(rw_timeout));
    let _ = stream.set_write_timeout(Some(rw_timeout));

    // Auth
    if let Err(e) = client_auth(&mut stream, secret) {
        let reason = categorize_auth_error(e);
        return WorkerStatus {
            addr: addr.to_string(),
            status: format!("auth-fail ({reason})"),
            pong: None,
            latency_ms: None,
        };
    }

    // PING/PONG
    let ping_payload = encode_ping();
    if let Err(_) = network::write_msg(&mut stream, MSG_PING, &ping_payload) {
        return WorkerStatus {
            addr: addr.to_string(),
            status: "offline (write error)".to_string(),
            pong: None,
            latency_ms: None,
        };
    }

    let rtt_start = Instant::now();
    let (msg_type, pong_payload) = match network::read_msg(&mut stream) {
        Ok(r) => r,
        Err(_) => return WorkerStatus {
            addr: addr.to_string(),
            status: "offline (read error)".to_string(),
            pong: None,
            latency_ms: None,
        },
    };
    let latency = rtt_start.elapsed().as_millis() as u64;

    if msg_type != MSG_PONG {
        return WorkerStatus {
            addr: addr.to_string(),
            status: format!("protocol-error (expected PONG, got 0x{msg_type:02x})"),
            pong: None,
            latency_ms: None,
        };
    }

    match decode_pong(&pong_payload) {
        Ok(pong) => WorkerStatus {
            addr: addr.to_string(),
            status: "online".to_string(),
            pong: Some(pong),
            latency_ms: Some(latency),
        },
        Err(_) => WorkerStatus {
            addr: addr.to_string(),
            status: "protocol-error (invalid PONG)".to_string(),
            pong: None,
            latency_ms: None,
        },
    }
}

/// Ping all workers and return their statuses.
pub fn colony_status(
    workers: &[String],
    secret: Option<&str>,
    connect_timeout_ms: u64,
) -> Vec<WorkerStatus> {
    workers.iter()
        .map(|addr| ping_worker(addr, secret, connect_timeout_ms))
        .collect()
}

/// Format colony status as a table.
pub fn format_status_table(statuses: &[WorkerStatus]) -> String {
    let mut out = String::new();
    out.push_str(&format!("Colony Status: {} worker{}\n\n",
        statuses.len(),
        if statuses.len() == 1 { "" } else { "s" },
    ));

    // Find max address length for column alignment
    let max_addr = statuses.iter()
        .map(|s| s.addr.len())
        .max()
        .unwrap_or(6)
        .max(6);

    out.push_str(&format!("{:<width$}  {:<12}{:>7}{:>8}{:>8}  {:<5}{}\n",
        "WORKER", "STATUS", "EVALS", "FAILED", "AVG_MS", "BUSY", "BACKEND",
        width = max_addr,
    ));

    for s in statuses {
        if let Some(ref pong) = s.pong {
            out.push_str(&format!("{:<width$}  {:<12}{:>7}{:>8}{:>7}ms  {:<5}{}\n",
                s.addr,
                s.status,
                pong.evals_completed,
                pong.evals_failed,
                pong.avg_eval_ms,
                if pong.busy { "yes" } else { "no" },
                pong.backend,
                width = max_addr,
            ));
        } else {
            out.push_str(&format!("{:<width$}  {:<12}{:>7}{:>8}{:>8}  {:<5}{}\n",
                s.addr,
                s.status,
                "--", "--", "--", "--", "--",
                width = max_addr,
            ));
        }
    }

    // Summary
    let online = statuses.iter().filter(|s| s.status == "online").count();
    let total_evals: u32 = statuses.iter()
        .filter_map(|s| s.pong.as_ref().map(|p| p.evals_completed))
        .sum();
    let total_failed: u32 = statuses.iter()
        .filter_map(|s| s.pong.as_ref().map(|p| p.evals_failed))
        .sum();
    let eval_times: Vec<u64> = statuses.iter()
        .filter_map(|s| s.pong.as_ref().map(|p| p.avg_eval_ms))
        .collect();
    let avg_eval = if eval_times.is_empty() { 0 } else {
        eval_times.iter().sum::<u64>() / eval_times.len() as u64
    };

    out.push_str(&format!("\nSummary: {}/{} online, {} evals completed, {} failed, avg {}ms\n",
        online, statuses.len(), total_evals, total_failed, avg_eval));

    out
}

/// Format colony status as JSON.
pub fn format_status_json(statuses: &[WorkerStatus]) -> String {
    let items: Vec<String> = statuses.iter().map(|s| {
        let mut fields: Vec<(&str, crate::json::JsonValue)> = vec![
            ("worker", crate::json::JsonValue::String(s.addr.clone())),
            ("status", crate::json::JsonValue::String(s.status.clone())),
        ];
        if let Some(ref pong) = s.pong {
            fields.push(("evals_completed", crate::json::JsonValue::Int(pong.evals_completed as i64)));
            fields.push(("evals_failed", crate::json::JsonValue::Int(pong.evals_failed as i64)));
            fields.push(("avg_eval_ms", crate::json::JsonValue::Int(pong.avg_eval_ms as i64)));
            fields.push(("busy", crate::json::JsonValue::Bool(pong.busy)));
            fields.push(("backend", crate::json::JsonValue::String(pong.backend.clone())));
        } else {
            fields.push(("evals_completed", crate::json::JsonValue::Null));
            fields.push(("evals_failed", crate::json::JsonValue::Null));
            fields.push(("avg_eval_ms", crate::json::JsonValue::Null));
            fields.push(("busy", crate::json::JsonValue::Null));
            fields.push(("backend", crate::json::JsonValue::Null));
        }
        if let Some(lat) = s.latency_ms {
            fields.push(("latency_ms", crate::json::JsonValue::Int(lat as i64)));
        } else {
            fields.push(("latency_ms", crate::json::JsonValue::Null));
        }
        format!("{}", crate::json::object(fields))
    }).collect();

    format!("[{}]", items.join(","))
}

/// Format a single worker ping result.
pub fn format_ping(status: &WorkerStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!("Pinging {}...\n", status.addr));
    out.push_str(&format!("  Status:     {}\n", status.status));

    if let Some(ref pong) = status.pong {
        out.push_str(&format!("  Evals:      {} completed, {} failed\n",
            pong.evals_completed, pong.evals_failed));
        out.push_str(&format!("  Avg eval:   {}ms\n", pong.avg_eval_ms));
        out.push_str(&format!("  Busy:       {}\n", if pong.busy { "yes" } else { "no" }));
        out.push_str(&format!("  Backend:    {}\n", pong.backend));
    }

    if let Some(lat) = status.latency_ms {
        out.push_str(&format!("  Latency:    {}ms\n", lat));
    }

    out
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

        let entry = res.to_arena_entry("test.ag", Some("10.0.0.1:9462"));
        assert_eq!(entry.file, "test.ag");
        assert!((entry.score - 0.75).abs() < 1e-10);
        assert_eq!(entry.prompt_count, 2);
        assert!(entry.error.is_none());
        assert_eq!(entry.worker.as_deref(), Some("10.0.0.1:9462"));
        assert_eq!(entry.eval_time_ms, Some(100));
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

        let entry = res.to_arena_entry("bad.ag", None);
        assert!(entry.error.is_some());
        assert!(entry.worker.is_none());
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

    // --- M33: Colony coordinator tests ---

    #[test]
    fn parse_workers_csv() {
        let workers = parse_workers("10.0.0.1:9462,10.0.0.2:9462,10.0.0.3:9462");
        assert_eq!(workers, vec!["10.0.0.1:9462", "10.0.0.2:9462", "10.0.0.3:9462"]);
    }

    #[test]
    fn parse_workers_csv_with_spaces() {
        let workers = parse_workers("host1:9462, host2:9462 , host3:9462");
        assert_eq!(workers, vec!["host1:9462", "host2:9462", "host3:9462"]);
    }

    #[test]
    fn parse_workers_from_file() {
        let dir = std::env::temp_dir().join(format!("agentis_test_workers_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("workers.txt");
        std::fs::write(&file, "10.0.0.1:9462\n# comment\n10.0.0.2:9462\n\n10.0.0.3:9462\n").unwrap();

        let workers = parse_workers(file.to_str().unwrap());
        assert_eq!(workers, vec!["10.0.0.1:9462", "10.0.0.2:9462", "10.0.0.3:9462"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_workers_empty() {
        let workers = parse_workers("");
        assert!(workers.is_empty());
    }

    #[test]
    fn parse_workers_single() {
        let workers = parse_workers("localhost:9462");
        assert_eq!(workers, vec!["localhost:9462"]);
    }

    #[test]
    fn categorize_connect_error_refused() {
        let err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let msg = categorize_connect_error(err);
        assert!(msg.contains("connection refused"), "got: {msg}");
    }

    #[test]
    fn categorize_connect_error_timeout() {
        let err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let msg = categorize_connect_error(err);
        assert!(msg.contains("timed out"), "got: {msg}");
    }

    #[test]
    fn categorize_auth_error_msg() {
        let err = NetworkError::Protocol("authentication failed".to_string());
        let msg = categorize_auth_error(err);
        assert_eq!(msg, "auth failed");
    }

    #[test]
    fn categorize_protocol_error_timeout() {
        let err = NetworkError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));
        let msg = categorize_protocol_error(err);
        assert_eq!(msg, "timed out");
    }

    #[test]
    fn evaluate_on_worker_connection_refused() {
        // Try connecting to a port that's not listening
        let result = evaluate_on_worker(
            "127.0.0.1:1",   // port 1 should be refused
            "test.ag",
            "let x = 1;",
            10_000,
            &crate::fitness::FitnessWeights::default(),
            false,
            None,
            1_000, // 1s timeout
            5_000,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("refused") || err.contains("unreachable"), "got: {err}");
    }

    #[test]
    fn colony_eval_with_worker_and_fallback() {
        // Start a real worker, send an EVAL, verify result
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        // Spawn a minimal "worker" that handles one EVAL and returns a canned RESULT
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            // Read EVAL
            let (msg_type, payload) = crate::network::read_msg(&mut stream).unwrap();
            assert_eq!(msg_type, crate::network::MSG_EVAL);
            let req = decode_eval_request(&payload).unwrap();
            assert_eq!(req.filename, "test.ag");

            // Send back a success RESULT
            let result = EvalResult {
                request_id: req.request_id,
                status: STATUS_OK,
                score: 0.85,
                cb_eff: 0.9,
                val_rate: 1.0,
                exp_rate: 0.5,
                prompt_count: 2,
                output: "hello".to_string(),
                error: String::new(),
                eval_time_ms: 42,
            };
            let payload = encode_eval_result(&result);
            crate::network::write_msg(&mut stream, crate::network::MSG_RESULT, &payload).unwrap();
        });

        let entry = evaluate_on_worker(
            &addr,
            "test.ag",
            "let x = 1;",
            10_000,
            &crate::fitness::FitnessWeights::default(),
            false,
            None,
            5_000,
            30_000,
        ).unwrap();

        assert_eq!(entry.file, "test.ag");
        assert!((entry.score - 0.85).abs() < 1e-10);
        assert_eq!(entry.worker.as_deref(), Some(&*addr));
        assert_eq!(entry.eval_time_ms, Some(42));
        assert!(entry.error.is_none());

        handle.join().unwrap();
    }

    #[test]
    fn colony_eval_with_auth() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let secret = "test-colony-secret";

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            // Auth handshake
            server_auth(&mut stream, Some(secret)).unwrap();

            // Read EVAL
            let (msg_type, payload) = crate::network::read_msg(&mut stream).unwrap();
            assert_eq!(msg_type, crate::network::MSG_EVAL);
            let req = decode_eval_request(&payload).unwrap();

            // Send RESULT
            let result = EvalResult {
                request_id: req.request_id,
                status: STATUS_OK,
                score: 0.7,
                cb_eff: 0.8,
                val_rate: 0.5,
                exp_rate: 1.0,
                prompt_count: 1,
                output: String::new(),
                error: String::new(),
                eval_time_ms: 50,
            };
            let payload = encode_eval_result(&result);
            crate::network::write_msg(&mut stream, crate::network::MSG_RESULT, &payload).unwrap();
        });

        let entry = evaluate_on_worker(
            &addr,
            "auth.ag",
            "let x = 1;",
            10_000,
            &crate::fitness::FitnessWeights::default(),
            false,
            Some(secret),
            5_000,
            30_000,
        ).unwrap();

        assert!((entry.score - 0.7).abs() < 1e-10);
        handle.join().unwrap();
    }

    #[test]
    fn colony_eval_auth_failure() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = server_auth(&mut stream, Some("correct-secret"));
        });

        let result = evaluate_on_worker(
            &addr,
            "test.ag",
            "let x = 1;",
            10_000,
            &crate::fitness::FitnessWeights::default(),
            false,
            Some("wrong-secret"),
            5_000,
            30_000,
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("auth failed"), "got: {err}");

        handle.join().unwrap();
    }

    #[test]
    fn colony_stats_format() {
        let entries = vec![
            crate::arena::ArenaEntry {
                file: "a.ag".to_string(),
                score: 0.9, cb_eff: 0.95, val_rate: 1.0, exp_rate: 0.5,
                prompt_count: 3, error: None, rounds: 1,
                worker: Some("10.0.0.1:9462".to_string()),
                eval_time_ms: Some(100),
            },
            crate::arena::ArenaEntry {
                file: "b.ag".to_string(),
                score: 0.8, cb_eff: 0.9, val_rate: 1.0, exp_rate: 0.5,
                prompt_count: 2, error: None, rounds: 1,
                worker: Some("local".to_string()),
                eval_time_ms: Some(200),
            },
        ];
        let stats = crate::arena::format_colony_stats(&entries, 2);
        assert!(stats.contains("2 workers"), "got: {stats}");
        assert!(stats.contains("1 local fallback"), "got: {stats}");
        assert!(stats.contains("avg eval 150ms"), "got: {stats}");
    }

    // --- M34: Colony observability tests ---

    #[test]
    fn ping_worker_offline() {
        // Port 1 should be unreachable
        let status = ping_worker("127.0.0.1:1", None, 1_000);
        assert!(status.status.starts_with("offline"), "got: {}", status.status);
        assert!(status.pong.is_none());
        assert!(status.latency_ms.is_none());
    }

    #[test]
    fn ping_worker_invalid_address() {
        let status = ping_worker("not-a-valid-addr", None, 1_000);
        assert_eq!(status.status, "invalid-address");
    }

    #[test]
    fn ping_worker_online() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Read PING, send PONG
            let (msg_type, payload) = crate::network::read_msg(&mut stream).unwrap();
            assert_eq!(msg_type, crate::network::MSG_PING);
            let echo_ts = decode_ping(&payload).unwrap();
            let stats = WorkerStats::new();
            stats.evals_completed.store(42, Ordering::Relaxed);
            stats.evals_failed.store(3, Ordering::Relaxed);
            stats.total_eval_ms.store(4200, Ordering::Relaxed);
            let pong = encode_pong(echo_ts, &stats, "mock");
            crate::network::write_msg(&mut stream, crate::network::MSG_PONG, &pong).unwrap();
        });

        let status = ping_worker(&addr, None, 5_000);
        assert_eq!(status.status, "online");
        assert!(status.latency_ms.is_some());

        let pong = status.pong.unwrap();
        assert_eq!(pong.evals_completed, 42);
        assert_eq!(pong.evals_failed, 3);
        assert_eq!(pong.avg_eval_ms, 100); // 4200 / 42
        assert_eq!(pong.backend, "mock");

        handle.join().unwrap();
    }

    #[test]
    fn ping_worker_with_auth() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let secret = "ping-secret";

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            server_auth(&mut stream, Some(secret)).unwrap();
            let (_, payload) = crate::network::read_msg(&mut stream).unwrap();
            let echo_ts = decode_ping(&payload).unwrap();
            let stats = WorkerStats::new();
            let pong = encode_pong(echo_ts, &stats, "test");
            crate::network::write_msg(&mut stream, crate::network::MSG_PONG, &pong).unwrap();
        });

        let status = ping_worker(&addr, Some(secret), 5_000);
        assert_eq!(status.status, "online");

        handle.join().unwrap();
    }

    #[test]
    fn ping_worker_auth_failure() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = server_auth(&mut stream, Some("right-secret"));
        });

        let status = ping_worker(&addr, Some("wrong-secret"), 5_000);
        assert!(status.status.starts_with("auth-fail"), "got: {}", status.status);
        assert!(status.pong.is_none());

        handle.join().unwrap();
    }

    #[test]
    fn colony_status_mixed() {
        use std::net::TcpListener;

        // One online worker
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let online_addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (_, payload) = crate::network::read_msg(&mut stream).unwrap();
            let echo_ts = decode_ping(&payload).unwrap();
            let stats = WorkerStats::new();
            stats.evals_completed.store(10, Ordering::Relaxed);
            let pong = encode_pong(echo_ts, &stats, "mock");
            crate::network::write_msg(&mut stream, crate::network::MSG_PONG, &pong).unwrap();
        });

        let workers = vec![online_addr.clone(), "127.0.0.1:1".to_string()];
        let statuses = colony_status(&workers, None, 2_000);

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].status, "online");
        assert!(statuses[1].status.starts_with("offline"));

        handle.join().unwrap();
    }

    #[test]
    fn format_status_table_output() {
        let statuses = vec![
            WorkerStatus {
                addr: "10.0.0.1:9462".to_string(),
                status: "online".to_string(),
                pong: Some(PongData {
                    echo_ts: 0,
                    evals_completed: 50,
                    evals_failed: 2,
                    avg_eval_ms: 150,
                    busy: false,
                    backend: "http".to_string(),
                }),
                latency_ms: Some(12),
            },
            WorkerStatus {
                addr: "10.0.0.2:9462".to_string(),
                status: "offline (connection refused)".to_string(),
                pong: None,
                latency_ms: None,
            },
        ];

        let table = format_status_table(&statuses);
        assert!(table.contains("Colony Status: 2 workers"), "got: {table}");
        assert!(table.contains("10.0.0.1:9462"), "got: {table}");
        assert!(table.contains("online"), "got: {table}");
        assert!(table.contains("50"), "got: {table}");
        assert!(table.contains("http"), "got: {table}");
        assert!(table.contains("10.0.0.2:9462"), "got: {table}");
        assert!(table.contains("offline"), "got: {table}");
        assert!(table.contains("1/2 online"), "got: {table}");
        // Summary averages worker eval times (avg_eval_ms), not PING latencies
        assert!(table.contains("avg 150ms"), "got: {table}");
    }

    #[test]
    fn format_status_json_output() {
        let statuses = vec![
            WorkerStatus {
                addr: "10.0.0.1:9462".to_string(),
                status: "online".to_string(),
                pong: Some(PongData {
                    echo_ts: 0,
                    evals_completed: 10,
                    evals_failed: 1,
                    avg_eval_ms: 100,
                    busy: true,
                    backend: "cli".to_string(),
                }),
                latency_ms: Some(5),
            },
        ];

        let json = format_status_json(&statuses);
        assert!(json.contains("\"worker\":\"10.0.0.1:9462\""), "got: {json}");
        assert!(json.contains("\"status\":\"online\""), "got: {json}");
        assert!(json.contains("\"evals_completed\":10"), "got: {json}");
        assert!(json.contains("\"busy\":true"), "got: {json}");
        assert!(json.contains("\"backend\":\"cli\""), "got: {json}");
        assert!(json.contains("\"latency_ms\":5"), "got: {json}");
    }

    #[test]
    fn format_status_json_offline() {
        let statuses = vec![
            WorkerStatus {
                addr: "10.0.0.1:9462".to_string(),
                status: "offline".to_string(),
                pong: None,
                latency_ms: None,
            },
        ];

        let json = format_status_json(&statuses);
        assert!(json.contains("\"evals_completed\":null"), "got: {json}");
        assert!(json.contains("\"latency_ms\":null"), "got: {json}");
    }

    #[test]
    fn format_ping_output() {
        let status = WorkerStatus {
            addr: "10.0.0.1:9462".to_string(),
            status: "online".to_string(),
            pong: Some(PongData {
                echo_ts: 0,
                evals_completed: 100,
                evals_failed: 5,
                avg_eval_ms: 200,
                busy: false,
                backend: "http".to_string(),
            }),
            latency_ms: Some(8),
        };

        let output = format_ping(&status);
        assert!(output.contains("Pinging 10.0.0.1:9462"), "got: {output}");
        assert!(output.contains("Status:     online"), "got: {output}");
        assert!(output.contains("100 completed, 5 failed"), "got: {output}");
        assert!(output.contains("Avg eval:   200ms"), "got: {output}");
        assert!(output.contains("Busy:       no"), "got: {output}");
        assert!(output.contains("Backend:    http"), "got: {output}");
        assert!(output.contains("Latency:    8ms"), "got: {output}");
    }

    #[test]
    fn format_ping_offline() {
        let status = WorkerStatus {
            addr: "10.0.0.1:9462".to_string(),
            status: "offline (connection refused)".to_string(),
            pong: None,
            latency_ms: None,
        };

        let output = format_ping(&status);
        assert!(output.contains("offline"), "got: {output}");
        assert!(!output.contains("Evals:"), "got: {output}");
        assert!(!output.contains("Latency:"), "got: {output}");
    }
}
