// Host synchronization for the production traffic budget and its tests.
mod sync {
    pub struct IrqSpinLock<T>(std::sync::Mutex<T>);
    impl<T> IrqSpinLock<T> {
        pub fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }
        pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
            self.0.lock().unwrap()
        }
    }
}
mod traffic {
    /* PRODUCTION_TRAFFIC */
}
