//! POSIX thread lifecycle and thread-specific storage over Scarlet Rust std.
//!
//! One matching Rust runtime owns OS threads and TLS. C handles use Rust's
//! process-unique thread IDs, never pointers into a reusable allocation. Key
//! values live outside Rust TLS so a C destructor can call setspecific while
//! the Rust TLS cleanup marker is being destroyed.

use std::ffi::{c_int, c_uint, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

const ESRCH: c_int = 3;
const EAGAIN: c_int = 11;
const ENOMEM: c_int = 12;
const EINVAL: c_int = 22;
const EDEADLK: c_int = 35;
pub const PTHREAD_CREATE_JOINABLE: c_int = 0;
pub const PTHREAD_CREATE_DETACHED: c_int = 1;
pub const PTHREAD_STACK_MIN: usize = 65536;
pub const PTHREAD_KEYS_MAX: usize = 1024;
pub const PTHREAD_DESTRUCTOR_ITERATIONS: usize = 4;
const DEFAULT_STACK: usize = 2 * 1024 * 1024;
const ATTR_MAGIC: u32 = 0x5343_5041;
const ONCE_DONE: usize = usize::MAX;

type Start = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type Destructor = unsafe extern "C" fn(*mut c_void);

/// Every pthread entry point restores errno, including after std operations
/// and user callbacks. POSIX pthread errors are returned directly.
struct PreserveErrno(c_int);
impl PreserveErrno {
    fn new() -> Self {
        // SAFETY: errno is owned by the calling thread, including TLS cleanup.
        Self(unsafe { *crate::__errno_location() })
    }
}
impl Drop for PreserveErrno {
    fn drop(&mut self) {
        // SAFETY: the guard cannot outlive this thread.
        unsafe { *crate::__errno_location() = self.0 };
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Rust IDs are unique across all threads in the process, including threads
/// created by Rust rather than pthread_create. Rust keeps identity available
/// throughout TLS destruction; the matching Scarlet ABI is LP64.
pub(crate) fn current_id() -> usize {
    std::thread::current_id().as_u64().get() as usize
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PthreadAttr {
    pub stack_size: usize,
    pub detach_state: c_int,
    pub magic: u32,
}

impl PthreadAttr {
    const DEFAULT: Self = Self {
        stack_size: DEFAULT_STACK,
        detach_state: PTHREAD_CREATE_JOINABLE,
        magic: ATTR_MAGIC,
    };

    fn valid(&self) -> bool {
        self.magic == ATTR_MAGIC
            && matches!(
                self.detach_state,
                PTHREAD_CREATE_JOINABLE | PTHREAD_CREATE_DETACHED
            )
            && valid_stack(self.stack_size)
    }
}

fn valid_stack(size: usize) -> bool {
    // The Native backend reserves an additional guard page. Require page-sized
    // stacks, and leave room for its rounding and guard arithmetic.
    size >= PTHREAD_STACK_MIN && size <= isize::MAX as usize - 8192 && size % 4096 == 0
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Joinable,
    Joining,
    Detached,
}
struct ThreadRecord {
    id: usize,
    handle: Option<JoinHandle<usize>>,
    phase: Phase,
    finished: bool,
}
static THREADS: Mutex<Vec<ThreadRecord>> = Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct Key {
    id: c_uint,
    destructor: Option<Destructor>,
}
struct LocalValues {
    id: usize,
    values: Vec<(c_uint, usize)>,
}
struct Specific {
    next_key: c_uint,
    keys: Vec<Key>,
    locals: Vec<LocalValues>,
}
static SPECIFIC: Mutex<Specific> = Mutex::new(Specific {
    next_key: 1,
    keys: Vec::new(),
    locals: Vec::new(),
});

struct ThreadCleanup(usize);
std::thread_local! {
    static CLEANUP: ThreadCleanup = ThreadCleanup(current_id());
}
impl Drop for ThreadCleanup {
    fn drop(&mut self) {
        cleanup_specific(self.0);
        let mut threads = lock(&THREADS);
        if let Some(index) = threads.iter().position(|thread| thread.id == self.0) {
            threads[index].finished = true;
            if threads[index].phase == Phase::Detached {
                threads.swap_remove(index);
            }
        }
    }
}

fn cleanup_specific(id: usize) {
    for _ in 0..PTHREAD_DESTRUCTOR_ITERATIONS {
        // Snapshot identities, not values or callbacks: key_delete from one
        // callback suppresses a later callback, and the other keys' values
        // remain visible until their own callback starts. No allocation here.
        let mut keys = [0; PTHREAD_KEYS_MAX];
        let count = {
            let state = lock(&SPECIFIC);
            let Some(local) = state.locals.iter().find(|local| local.id == id) else {
                return;
            };
            for (out, (key, _)) in keys.iter_mut().zip(&local.values) {
                *out = *key;
            }
            local.values.len()
        };
        let mut called = false;
        for key in keys.into_iter().take(count) {
            let callback = {
                let mut state = lock(&SPECIFIC);
                let destructor = state
                    .keys
                    .iter()
                    .find(|item| item.id == key)
                    .and_then(|item| item.destructor);
                let Some(local) = state.locals.iter_mut().find(|local| local.id == id) else {
                    break;
                };
                let value = local
                    .values
                    .iter_mut()
                    .find(|item| item.0 == key)
                    .map(|item| std::mem::replace(&mut item.1, 0))
                    .unwrap_or(0);
                destructor
                    .filter(|_| value != 0)
                    .map(|destructor| (destructor, value))
            };
            if let Some((destructor, value)) = callback {
                called = true;
                // SAFETY: key_create's caller supplied a C callback accepting
                // its thread-specific value; no registry lock crosses it.
                unsafe { destructor(value as *mut c_void) };
            }
        }
        if !called {
            break;
        }
    }
    let mut state = lock(&SPECIFIC);
    if let Some(index) = state.locals.iter().position(|local| local.id == id) {
        // Values reinstalled on the last round are discarded without a fifth
        // callback, as permitted by PTHREAD_DESTRUCTOR_ITERATIONS.
        state.locals.swap_remove(index);
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_self() -> usize {
    let _errno = PreserveErrno::new();
    current_id()
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_equal(left: usize, right: usize) -> c_int {
    c_int::from(left == right)
}

/// # Safety
/// output must be writable; attr, if non-null, must be initialized. start and
/// argument must remain valid until the created thread finishes using them.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_create(
    output: *mut usize,
    attr: *const PthreadAttr,
    start: Option<Start>,
    argument: *mut c_void,
) -> c_int {
    let _errno = PreserveErrno::new();
    let Some(start) = start else {
        return EINVAL;
    };
    if output.is_null() {
        return EINVAL;
    }
    let attr = if attr.is_null() {
        PthreadAttr::DEFAULT
    } else {
        // SAFETY: caller supplies a live attr record.
        unsafe { *attr }
    };
    if !attr.valid() {
        return EINVAL;
    }
    let mut threads = lock(&THREADS);
    if threads.try_reserve(1).is_err() {
        return EAGAIN;
    }
    let argument = argument as usize;
    let handle = match std::thread::Builder::new()
        .stack_size(attr.stack_size)
        .spawn(move || {
            CLEANUP.with(|_| ());
            // SAFETY: the C caller is responsible for the callback and argument.
            unsafe { start(argument as *mut c_void) as usize }
        }) {
        Ok(handle) => handle,
        Err(_) => return EAGAIN,
    };
    let id = handle.thread().id().as_u64().get() as usize;
    let detached = attr.detach_state == PTHREAD_CREATE_DETACHED;
    threads.push(ThreadRecord {
        id,
        handle: if detached { None } else { Some(handle) },
        phase: if detached {
            Phase::Detached
        } else {
            Phase::Joinable
        },
        finished: false,
    });
    // Publishing under the same lock as the exit marker closes the
    // finish-before-create-returns race, including detached threads.
    unsafe { output.write(id) };
    0
}

/// # Safety
/// result, when non-null, must point to writable pointer-sized storage.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_join(id: usize, result: *mut *mut c_void) -> c_int {
    let _errno = PreserveErrno::new();
    if id == current_id() {
        return EDEADLK;
    }
    let handle = {
        let mut threads = lock(&THREADS);
        let Some(thread) = threads.iter_mut().find(|thread| thread.id == id) else {
            return ESRCH;
        };
        if thread.phase != Phase::Joinable {
            return EINVAL;
        }
        thread.phase = Phase::Joining;
        // Every Joinable record owns a handle, taken exactly once under lock.
        thread.handle.take()
    };
    let Some(handle) = handle else {
        return EINVAL;
    };
    let joined = handle.join();
    let mut threads = lock(&THREADS);
    if let Some(index) = threads.iter().position(|thread| thread.id == id) {
        threads.swap_remove(index);
    }
    match joined {
        Ok(value) => {
            if !result.is_null() {
                // SAFETY: supplied by the C caller.
                unsafe { result.write(value as *mut c_void) };
            }
            0
        }
        // A C callback cannot unwind across its C ABI. This branch only
        // handles a Rust backend panic in an unwinding host test build.
        Err(_) => EINVAL,
    }
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_detach(id: usize) -> c_int {
    let _errno = PreserveErrno::new();
    let handle = {
        let mut threads = lock(&THREADS);
        let Some(index) = threads.iter().position(|thread| thread.id == id) else {
            return ESRCH;
        };
        let thread = &mut threads[index];
        if thread.phase != Phase::Joinable {
            return EINVAL;
        }
        thread.phase = Phase::Detached;
        let handle = thread.handle.take();
        if thread.finished {
            threads.swap_remove(index);
        }
        handle
    };
    drop(handle);
    0
}

/// # Safety
/// attr points to writable storage for a PthreadAttr.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_init(attr: *mut PthreadAttr) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null() {
        return EINVAL;
    }
    unsafe { attr.write(PthreadAttr::DEFAULT) };
    0
}

/// # Safety
/// attr points to an initialized, exclusively accessible PthreadAttr.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_destroy(attr: *mut PthreadAttr) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null() || !unsafe { &*attr }.valid() {
        return EINVAL;
    }
    unsafe { (*attr).magic = 0 };
    0
}

/// # Safety
/// attr is initialized and value points to writable storage.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_getdetachstate(
    attr: *const PthreadAttr,
    value: *mut c_int,
) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null() || value.is_null() || !unsafe { &*attr }.valid() {
        return EINVAL;
    }
    unsafe { value.write((*attr).detach_state) };
    0
}

/// # Safety
/// attr is initialized and exclusively accessible.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_setdetachstate(
    attr: *mut PthreadAttr,
    value: c_int,
) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null()
        || !unsafe { &*attr }.valid()
        || !matches!(value, PTHREAD_CREATE_JOINABLE | PTHREAD_CREATE_DETACHED)
    {
        return EINVAL;
    }
    unsafe { (*attr).detach_state = value };
    0
}

/// # Safety
/// attr is initialized and value points to writable storage.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_getstacksize(
    attr: *const PthreadAttr,
    value: *mut usize,
) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null() || value.is_null() || !unsafe { &*attr }.valid() {
        return EINVAL;
    }
    unsafe { value.write((*attr).stack_size) };
    0
}

/// # Safety
/// attr is initialized and exclusively accessible.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_attr_setstacksize(attr: *mut PthreadAttr, value: usize) -> c_int {
    let _errno = PreserveErrno::new();
    if attr.is_null() || !unsafe { &*attr }.valid() || !valid_stack(value) {
        return EINVAL;
    }
    unsafe { (*attr).stack_size = value };
    0
}

#[repr(C)]
pub struct PthreadOnce {
    pub state: AtomicUsize,
}
static ONCE_LOCK: Mutex<()> = Mutex::new(());
static ONCE_WAKE: Condvar = Condvar::new();

/// # Safety
/// once points to a zero-initialized, live PthreadOnce; init is a C callback.
/// The object must remain allocated until all concurrent calls return.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_once(
    once: *mut PthreadOnce,
    init: Option<unsafe extern "C" fn()>,
) -> c_int {
    let _errno = PreserveErrno::new();
    let Some(init) = init else {
        return EINVAL;
    };
    if once.is_null() {
        return EINVAL;
    }
    let state = unsafe { &(*once).state };
    if state.load(Ordering::Acquire) == ONCE_DONE {
        return 0;
    }
    let id = current_id();
    loop {
        match state.compare_exchange(0, id, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) => {
                unsafe { init() };
                let _lock = lock(&ONCE_LOCK);
                state.store(ONCE_DONE, Ordering::Release);
                ONCE_WAKE.notify_all();
                return 0;
            }
            Err(ONCE_DONE) => return 0,
            Err(owner) if owner == id => return EDEADLK,
            Err(_) => {
                let mut guard = lock(&ONCE_LOCK);
                while state.load(Ordering::Acquire) != ONCE_DONE {
                    guard = ONCE_WAKE
                        .wait(guard)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                return 0;
            }
        }
    }
}

/// # Safety
/// output is writable and destructor, if supplied, accepts stored values.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_key_create(
    output: *mut c_uint,
    destructor: Option<Destructor>,
) -> c_int {
    let _errno = PreserveErrno::new();
    if output.is_null() {
        return EINVAL;
    }
    let mut state = lock(&SPECIFIC);
    if state.keys.len() == PTHREAD_KEYS_MAX || state.next_key == c_uint::MAX {
        return EAGAIN;
    }
    if state.keys.try_reserve(1).is_err() {
        return ENOMEM;
    }
    let id = state.next_key;
    state.next_key += 1;
    state.keys.push(Key { id, destructor });
    unsafe { output.write(id) };
    0
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_key_delete(key: c_uint) -> c_int {
    let _errno = PreserveErrno::new();
    let mut state = lock(&SPECIFIC);
    let Some(index) = state.keys.iter().position(|item| item.id == key) else {
        return EINVAL;
    };
    state.keys.swap_remove(index);
    // Deleting a key does not run its destructor. IDs are never reused, so an
    // old key cannot acquire a new key's values after deletion.
    for local in &mut state.locals {
        local.values.retain(|item| item.0 != key);
    }
    0
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_getspecific(key: c_uint) -> *mut c_void {
    let _errno = PreserveErrno::new();
    let id = current_id();
    let state = lock(&SPECIFIC);
    state
        .locals
        .iter()
        .find(|local| local.id == id)
        .and_then(|local| local.values.iter().find(|item| item.0 == key))
        .map_or(std::ptr::null_mut(), |item| item.1 as *mut c_void)
}

#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub extern "C" fn pthread_setspecific(key: c_uint, value: *const c_void) -> c_int {
    let _errno = PreserveErrno::new();
    let id = current_id();
    let mut state = lock(&SPECIFIC);
    if !state.keys.iter().any(|item| item.id == key) {
        return EINVAL;
    }
    let index = match state.locals.iter().position(|local| local.id == id) {
        Some(index) => index,
        None => {
            if value.is_null() {
                return 0;
            }
            // CLEANUP is touched only on first registration, never from a
            // destructor reentering this function: its record stays present
            // until all four callback rounds finish.
            if CLEANUP.try_with(|_| ()).is_err() {
                return EINVAL;
            }
            if state.locals.try_reserve(1).is_err() {
                return ENOMEM;
            }
            let index = state.locals.len();
            state.locals.push(LocalValues {
                id,
                values: Vec::new(),
            });
            index
        }
    };
    let local = &mut state.locals[index];
    if let Some(item) = local.values.iter_mut().find(|item| item.0 == key) {
        item.1 = value as usize;
    } else if !value.is_null() {
        if local.values.try_reserve(1).is_err() {
            return ENOMEM;
        }
        local.values.push((key, value as usize));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    unsafe extern "C" fn roundtrip(value: *mut c_void) -> *mut c_void {
        value
    }
    fn errno(value: c_int) {
        unsafe { *crate::__errno_location() = value };
    }
    fn get_errno() -> c_int {
        unsafe { *crate::__errno_location() }
    }

    #[test]
    fn create_join_and_identity_preserve_errno() {
        let _serial = lock(&TEST_LOCK);
        errno(123);
        let own = pthread_self();
        assert_ne!(own, 0);
        assert_eq!(pthread_self(), own);
        for value in 1..33usize {
            let mut id = 0;
            let mut result = std::ptr::null_mut();
            assert_eq!(
                unsafe {
                    pthread_create(&mut id, std::ptr::null(), Some(roundtrip), value as *mut _)
                },
                0
            );
            assert_ne!(id, own);
            assert_eq!(unsafe { pthread_join(id, &mut result) }, 0);
            assert_eq!(result as usize, value);
            assert_eq!(unsafe { pthread_join(id, &mut result) }, ESRCH);
            assert_eq!(get_errno(), 123);
        }
        assert_eq!(unsafe { pthread_join(own, std::ptr::null_mut()) }, EDEADLK);
        assert_eq!(pthread_detach(usize::MAX), ESRCH);
        assert_eq!(get_errno(), 123);
    }

    #[test]
    fn attributes_validate_and_apply_stack_and_detach() {
        let _serial = lock(&TEST_LOCK);
        let mut attr = PthreadAttr::DEFAULT;
        errno(77);
        assert_eq!(unsafe { pthread_attr_init(&mut attr) }, 0);
        for size in [0, PTHREAD_STACK_MIN - 1, PTHREAD_STACK_MIN + 1, usize::MAX] {
            assert_eq!(
                unsafe { pthread_attr_setstacksize(&mut attr, size) },
                EINVAL
            );
        }
        assert_eq!(
            unsafe { pthread_attr_setdetachstate(&mut attr, 42) },
            EINVAL
        );
        assert_eq!(
            unsafe { pthread_attr_setstacksize(&mut attr, 256 * 1024) },
            0
        );
        let mut size = 0;
        assert_eq!(unsafe { pthread_attr_getstacksize(&attr, &mut size) }, 0);
        assert_eq!(size, 256 * 1024);
        let mut id = 0;
        assert_eq!(
            unsafe { pthread_create(&mut id, &attr, Some(roundtrip), std::ptr::null_mut()) },
            0
        );
        assert_eq!(unsafe { pthread_join(id, std::ptr::null_mut()) }, 0);
        assert_eq!(
            unsafe { pthread_attr_setdetachstate(&mut attr, PTHREAD_CREATE_DETACHED) },
            0
        );
        let mut state = -1;
        assert_eq!(unsafe { pthread_attr_getdetachstate(&attr, &mut state) }, 0);
        assert_eq!(state, PTHREAD_CREATE_DETACHED);
        for _ in 0..32 {
            assert_eq!(
                unsafe { pthread_create(&mut id, &attr, Some(roundtrip), std::ptr::null_mut()) },
                0
            );
            await_unregistered(id);
            assert_eq!(unsafe { pthread_join(id, std::ptr::null_mut()) }, ESRCH);
        }
        assert_eq!(unsafe { pthread_attr_destroy(&mut attr) }, 0);
        assert_eq!(
            unsafe { pthread_create(&mut id, &attr, Some(roundtrip), std::ptr::null_mut()) },
            EINVAL
        );
        assert_eq!(get_errno(), 77);
    }

    fn await_unregistered(id: usize) {
        let start = Instant::now();
        while lock(&THREADS).iter().any(|thread| thread.id == id) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "thread cleanup stalled"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn detach_completed_or_running_threads_reclaims_registry() {
        let _serial = lock(&TEST_LOCK);
        for _ in 0..32 {
            let mut id = 0;
            assert_eq!(
                unsafe {
                    pthread_create(
                        &mut id,
                        std::ptr::null(),
                        Some(roundtrip),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            assert_eq!(pthread_detach(id), 0);
            await_unregistered(id);
            assert_eq!(pthread_detach(id), ESRCH);
        }
    }

    struct Gate {
        release: AtomicUsize,
    }
    unsafe extern "C" fn gated(value: *mut c_void) -> *mut c_void {
        let gate = unsafe { &*value.cast::<Gate>() };
        while gate.release.load(Ordering::Acquire) == 0 {
            std::thread::yield_now();
        }
        value
    }

    #[test]
    fn concurrent_join_and_detach_have_one_owner() {
        let _serial = lock(&TEST_LOCK);
        for _ in 0..32 {
            let gate = Box::new(Gate {
                release: AtomicUsize::new(0),
            });
            let mut id = 0;
            assert_eq!(
                unsafe {
                    pthread_create(
                        &mut id,
                        std::ptr::null(),
                        Some(gated),
                        (&*gate as *const Gate).cast_mut().cast(),
                    )
                },
                0
            );
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let left = barrier.clone();
            let joiner = std::thread::spawn(move || {
                left.wait();
                unsafe { pthread_join(id, std::ptr::null_mut()) }
            });
            let right = barrier.clone();
            let detacher = std::thread::spawn(move || {
                right.wait();
                pthread_detach(id)
            });
            barrier.wait();
            gate.release.store(1, Ordering::Release);
            let joined = joiner.join().unwrap();
            let detached = detacher.join().unwrap();
            assert_eq!((joined == 0) as u8 + (detached == 0) as u8, 1);
            assert!([0, EINVAL, ESRCH].contains(&joined));
            assert!([0, EINVAL, ESRCH].contains(&detached));
            await_unregistered(id);
        }
    }

    static ONCE_COUNT: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn initialize_once() {
        ONCE_COUNT.fetch_add(1, Ordering::Relaxed);
        errno(9);
        std::thread::sleep(Duration::from_millis(2));
    }
    #[test]
    fn once_serializes_and_restores_callback_errno() {
        let _serial = lock(&TEST_LOCK);
        ONCE_COUNT.store(0, Ordering::Relaxed);
        let once = Arc::new(PthreadOnce {
            state: AtomicUsize::new(0),
        });
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let once = once.clone();
                std::thread::spawn(move || {
                    errno(81);
                    assert_eq!(
                        unsafe {
                            pthread_once(Arc::as_ptr(&once).cast_mut(), Some(initialize_once))
                        },
                        0
                    );
                    assert_eq!(get_errno(), 81);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(ONCE_COUNT.load(Ordering::Relaxed), 1);
    }

    struct Reentrant {
        key: c_uint,
        count: AtomicUsize,
        errors: AtomicUsize,
    }
    unsafe extern "C" fn destructor(value: *mut c_void) {
        let context = unsafe { &*value.cast::<Reentrant>() };
        if !pthread_getspecific(context.key).is_null() {
            context.errors.fetch_add(1, Ordering::Relaxed);
        }
        context.count.fetch_add(1, Ordering::Relaxed);
        errno(79);
        if pthread_setspecific(context.key, value) != 0 || get_errno() != 79 || pthread_self() == 0
        {
            context.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe extern "C" fn set_key(value: *mut c_void) -> *mut c_void {
        let context = unsafe { &*value.cast::<Reentrant>() };
        errno(74);
        if pthread_setspecific(context.key, value) != 0
            || get_errno() != 74
            || pthread_getspecific(context.key) != value
        {
            context.errors.fetch_add(1, Ordering::Relaxed);
        }
        value
    }

    #[test]
    fn destructors_reenter_specific_apis_for_four_rounds() {
        let _serial = lock(&TEST_LOCK);
        let mut key = 0;
        assert_eq!(unsafe { pthread_key_create(&mut key, Some(destructor)) }, 0);
        let context = Box::new(Reentrant {
            key,
            count: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        });
        let ptr = (&*context as *const Reentrant).cast_mut().cast();
        let mut id = 0;
        assert_eq!(
            unsafe { pthread_create(&mut id, std::ptr::null(), Some(set_key), ptr) },
            0
        );
        assert_eq!(unsafe { pthread_join(id, std::ptr::null_mut()) }, 0);
        assert_eq!(
            context.count.load(Ordering::Relaxed),
            PTHREAD_DESTRUCTOR_ITERATIONS
        );
        assert_eq!(context.errors.load(Ordering::Relaxed), 0);
        assert!(pthread_getspecific(key).is_null());
        assert_eq!(pthread_key_delete(key), 0);
    }

    #[test]
    fn foreign_rust_thread_runs_pthread_destructors() {
        let _serial = lock(&TEST_LOCK);
        let mut key = 0;
        assert_eq!(unsafe { pthread_key_create(&mut key, Some(destructor)) }, 0);
        let context = Arc::new(Reentrant {
            key,
            count: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        });
        let child = context.clone();
        let id = std::thread::spawn(move || {
            unsafe { set_key(Arc::as_ptr(&child).cast_mut().cast()) };
            pthread_self()
        })
        .join()
        .unwrap();
        assert_ne!(id, pthread_self());
        assert_eq!(
            context.count.load(Ordering::Relaxed),
            PTHREAD_DESTRUCTOR_ITERATIONS
        );
        assert_eq!(context.errors.load(Ordering::Relaxed), 0);
        assert_eq!(pthread_key_delete(key), 0);
    }

    #[test]
    fn key_delete_does_not_call_destructor_or_reuse_identity() {
        let _serial = lock(&TEST_LOCK);
        let mut key = 0;
        assert_eq!(unsafe { pthread_key_create(&mut key, Some(destructor)) }, 0);
        assert_eq!(pthread_setspecific(key, 1usize as *const _), 0);
        assert_eq!(pthread_key_delete(key), 0);
        assert!(pthread_getspecific(key).is_null());
        assert_eq!(pthread_setspecific(key, std::ptr::null()), EINVAL);
        let mut replacement = 0;
        assert_eq!(unsafe { pthread_key_create(&mut replacement, None) }, 0);
        assert_ne!(key, replacement);
        assert!(pthread_getspecific(replacement).is_null());
        assert_eq!(pthread_key_delete(replacement), 0);
    }

    #[test]
    fn direct_errors_do_not_mutate_errno_or_outputs() {
        let _serial = lock(&TEST_LOCK);
        errno(39);
        let mut id = 314;
        assert_eq!(
            unsafe { pthread_create(&mut id, std::ptr::null(), None, std::ptr::null_mut()) },
            EINVAL
        );
        assert_eq!(id, 314);
        assert_eq!(unsafe { pthread_attr_init(std::ptr::null_mut()) }, EINVAL);
        assert_eq!(
            unsafe { pthread_once(std::ptr::null_mut(), Some(initialize_once)) },
            EINVAL
        );
        assert_eq!(
            unsafe { pthread_key_create(std::ptr::null_mut(), None) },
            EINVAL
        );
        assert_eq!(pthread_key_delete(0), EINVAL);
        assert_eq!(pthread_setspecific(0, std::ptr::null()), EINVAL);
        assert_eq!(get_errno(), 39);
    }

    #[test]
    fn key_limit_is_recoverable_and_deleted_slots_are_reusable() {
        let _serial = lock(&TEST_LOCK);
        let mut keys = [0; PTHREAD_KEYS_MAX];
        for key in &mut keys {
            assert_eq!(unsafe { pthread_key_create(key, None) }, 0);
        }
        errno(91);
        let mut extra = 42;
        assert_eq!(unsafe { pthread_key_create(&mut extra, None) }, EAGAIN);
        assert_eq!(extra, 42);
        assert_eq!(get_errno(), 91);
        let deleted = keys[0];
        assert_eq!(pthread_key_delete(deleted), 0);
        assert_eq!(unsafe { pthread_key_create(&mut keys[0], None) }, 0);
        assert_ne!(keys[0], deleted);
        for key in keys {
            assert_eq!(pthread_key_delete(key), 0);
        }
    }

    struct DeleteDuringCleanup {
        first: c_uint,
        second: c_uint,
        first_calls: AtomicUsize,
        second_calls: AtomicUsize,
        errors: AtomicUsize,
    }
    unsafe extern "C" fn delete_other_key(value: *mut c_void) {
        let context = unsafe { &*value.cast::<DeleteDuringCleanup>() };
        context.first_calls.fetch_add(1, Ordering::Relaxed);
        if pthread_getspecific(context.second) != value || pthread_key_delete(context.second) != 0 {
            context.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
    unsafe extern "C" fn deleted_destructor(value: *mut c_void) {
        let context = unsafe { &*value.cast::<DeleteDuringCleanup>() };
        context.second_calls.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "C" fn set_two_keys(value: *mut c_void) -> *mut c_void {
        let context = unsafe { &*value.cast::<DeleteDuringCleanup>() };
        if pthread_setspecific(context.first, value) != 0
            || pthread_setspecific(context.second, value) != 0
        {
            context.errors.fetch_add(1, Ordering::Relaxed);
        }
        value
    }

    #[test]
    fn destructor_observes_then_deletes_other_keys_without_stale_callbacks() {
        let _serial = lock(&TEST_LOCK);
        let mut first = 0;
        let mut second = 0;
        assert_eq!(
            unsafe { pthread_key_create(&mut first, Some(delete_other_key)) },
            0
        );
        assert_eq!(
            unsafe { pthread_key_create(&mut second, Some(deleted_destructor)) },
            0
        );
        let context = Box::new(DeleteDuringCleanup {
            first,
            second,
            first_calls: AtomicUsize::new(0),
            second_calls: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        });
        let mut id = 0;
        assert_eq!(
            unsafe {
                pthread_create(
                    &mut id,
                    std::ptr::null(),
                    Some(set_two_keys),
                    (&*context as *const DeleteDuringCleanup).cast_mut().cast(),
                )
            },
            0
        );
        assert_eq!(unsafe { pthread_join(id, std::ptr::null_mut()) }, 0);
        assert_eq!(context.first_calls.load(Ordering::Relaxed), 1);
        assert_eq!(context.second_calls.load(Ordering::Relaxed), 0);
        assert_eq!(context.errors.load(Ordering::Relaxed), 0);
        assert_eq!(pthread_key_delete(first), 0);
    }
}
