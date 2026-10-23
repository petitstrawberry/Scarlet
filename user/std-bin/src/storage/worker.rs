//! One scan thread, one replaceable request, one result: refresh cannot pile up work.
use super::scan::{self, Limits, Snapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct Request {
    generation: u64,
    path: PathBuf,
    cancel: Arc<AtomicBool>,
}
pub struct Response {
    pub generation: u64,
    pub result: Result<Snapshot, String>,
}
#[derive(Default)]
struct Shared {
    generation: u64,
    pending: Option<Request>,
    cancel: Option<Arc<AtomicBool>>,
    result: Option<Response>,
    closed: bool,
}
pub struct Worker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
}
impl Worker {
    pub fn new() -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let state = shared.clone();
        thread::Builder::new()
            .name("storage-scan".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let (lock, wake) = &*state;
                loop {
                    let request = {
                        let mut state = lock.lock().unwrap();
                        while state.pending.is_none() && !state.closed {
                            state = wake.wait(state).unwrap();
                        }
                        if state.closed {
                            return;
                        }
                        state.pending.take().unwrap()
                    };
                    let result = scan::scan(&request.path, &request.cancel, Limits::default());
                    let mut state = lock.lock().unwrap();
                    if !state.closed && state.generation == request.generation {
                        state.result = Some(Response {
                            generation: request.generation,
                            result,
                        });
                    }
                }
            })?;
        Ok(Self { shared })
    }
    pub fn start(&self, path: PathBuf) -> u64 {
        let (lock, wake) = &*self.shared;
        let mut state = lock.lock().unwrap();
        if let Some(cancel) = &state.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        state.cancel = Some(cancel.clone());
        state.pending = Some(Request {
            generation,
            path,
            cancel,
        });
        state.result = None;
        wake.notify_one();
        generation
    }
    pub fn cancel(&self) {
        if let Some(cancel) = &self.shared.0.lock().unwrap().cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn take_result(&self) -> Option<Response> {
        self.shared.0.lock().unwrap().result.take()
    }
    pub fn shutdown(&self) {
        let mut state = self.shared.0.lock().unwrap();
        state.closed = true;
        state.pending = None;
        if let Some(cancel) = &state.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.shared.1.notify_one();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_requests_publish_only_the_latest_generation() {
        let worker = Worker::new().unwrap();
        for _ in 0..20 {
            worker.start(std::env::temp_dir());
        }
        worker.cancel();
        let last = worker.start(PathBuf::from("/scarlet-storage-test-path-does-not-exist"));
        let start = std::time::Instant::now();
        loop {
            if let Some(response) = worker.take_result() {
                assert_eq!(response.generation, last);
                assert!(response.result.is_err());
                break;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
