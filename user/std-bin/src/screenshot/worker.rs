//! Stack budget for screenshot capture and PNG encoding workers.
//!
//! Use an explicit budget for native capture and PNG codec call chains instead
//! of relying on the platform default. The fallible API lets callers recover.
pub const STACK_SIZE: usize = 2 * 1024 * 1024;

pub fn spawn<F, T>(name: &str, work: F) -> std::io::Result<std::thread::JoinHandle<T>>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(STACK_SIZE)
        .spawn(work)
}
