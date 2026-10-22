//! Bounded worker stacks for file codecs and recursive filesystem operations.
//!
//! Scarlet's standard thread stack is 64 KiB. Deflate decoder construction and
//! the archive copy buffer exceed that in unoptimized native builds. Keep the
//! allocation explicit and fallible instead of leaving a worker silently stuck.
pub const STACK_BYTES: usize = 2 * 1024 * 1024;

pub fn spawn<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<T>> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .stack_size(STACK_BYTES)
        .spawn(work)
}
