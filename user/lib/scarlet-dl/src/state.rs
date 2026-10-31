use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char};
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

struct ErrorState {
    pending: Option<CString>,
    returned: Option<CString>,
}

// The matching native Rust runtime namespaces keys by the owning std image.
// These keys therefore cannot alias a loaded executable's Rust TLS keys.
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ERROR: RefCell<ErrorState> = const { RefCell::new(ErrorState {
        pending: None,
        returned: None,
    }) };
}

pub(crate) fn fail(error: impl std::fmt::Display) {
    let message = error.to_string().replace('\0', "?");
    ERROR.with_borrow_mut(|state| state.pending = Some(CString::new(message).unwrap()));
}

pub(crate) fn take_error() -> *mut c_char {
    ERROR.with_borrow_mut(|state| {
        state.returned = state.pending.take();
        state
            .returned
            .as_ref()
            .map_or(std::ptr::null_mut(), |s| s.as_ptr().cast_mut())
    })
}

pub(crate) struct Guard<'a, T>(MutexGuard<'a, T>);

impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        ACTIVE.set(false);
    }
}

pub(crate) fn lock<T>(runtime: &Mutex<T>) -> Result<Guard<'_, T>, &'static str> {
    if ACTIVE.replace(true) {
        return Err("dynamic loader constructor reentry is unsupported");
    }
    // Wait for another thread's operation. Same-thread reentry was rejected
    // above before it could deadlock on the non-recursive mutex.
    match runtime.lock() {
        Ok(guard) => Ok(Guard(guard)),
        Err(_) => {
            ACTIVE.set(false);
            Err("dynamic loader state is poisoned")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    #[test]
    fn concurrent_operations_wait_and_reentry_fails() {
        let runtime = Arc::new(Mutex::new(0));
        let mut first = lock(&runtime).unwrap();
        assert!(lock(&runtime).is_err());
        let (sender, receiver) = mpsc::channel();
        let worker_runtime = runtime.clone();
        let worker = std::thread::spawn(move || {
            sender.send("waiting").unwrap();
            let mut second = lock(&worker_runtime).unwrap();
            assert_eq!(*second, 1);
            *second = 2;
            sender.send("acquired").unwrap();
        });
        assert_eq!(receiver.recv().unwrap(), "waiting");
        assert!(receiver.recv_timeout(Duration::from_millis(30)).is_err());
        *first = 1;
        drop(first);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            "acquired"
        );
        worker.join().unwrap();
        assert_eq!(*lock(&runtime).unwrap(), 2);
    }

    #[test]
    fn errors_are_thread_local_and_consumed_once() {
        let barrier = Arc::new(Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|index| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let message = format!("thread {index}");
                    fail(&message);
                    barrier.wait();
                    let error = take_error();
                    assert_eq!(unsafe { CStr::from_ptr(error) }.to_str().unwrap(), message);
                    // Setting another error must not invalidate the last returned
                    // pointer until this thread calls dlerror again.
                    fail("next");
                    barrier.wait();
                    assert_eq!(unsafe { CStr::from_ptr(error) }.to_str().unwrap(), message);
                    assert_eq!(unsafe { CStr::from_ptr(take_error()) }.to_bytes(), b"next");
                    assert!(take_error().is_null());
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(take_error().is_null());
    }
}
