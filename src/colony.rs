// Distributed Colony infrastructure for agent evolution (Phase 8).
//
// M31: Thread pool for parallel local arena evaluation.
//       Future milestones (M32–M34) will add worker nodes,
//       colony coordination, and observability.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
}
