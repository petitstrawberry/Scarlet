//! Process-private pthread synchronization backed by Rust std.
//!
//! C objects contain monotonically allocated registry identities, not raw heap
//! pointers. Lookup clones an Arc under the registry lock; destruction rejects
//! outstanding mutex operations before removing their identity. Conditions can
//! be destroyed after notification while unblocked waiters retain the backend.
//! Rust MutexGuards never cross the public C API boundary.

use crate::Timespec;
use std::ffi::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

const EPERM: c_int = 1;
const EAGAIN: c_int = 11;
const ENOMEM: c_int = 12;
const EBUSY: c_int = 16;
const EINVAL: c_int = 22;
const EDEADLK: c_int = 35;
const ENOTSUP: c_int = 95;
const ETIMEDOUT: c_int = 110;
const NORMAL: c_int = 0;
const RECURSIVE: c_int = 1;
const ERRORCHECK: c_int = 2;
const DEAD: usize = usize::MAX;

struct PreserveErrno(c_int);
impl PreserveErrno {
    fn new() -> Self {
        // SAFETY: errno belongs to this thread and is initialized by the CRT.
        Self(unsafe { *crate::__errno_location() })
    }
}
impl Drop for PreserveErrno {
    fn drop(&mut self) {
        // SAFETY: the same calling thread owns this cell throughout the call.
        unsafe { *crate::__errno_location() = self.0 };
    }
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[repr(C)]
pub struct PthreadMutex {
    handle: AtomicUsize,
}
#[repr(C)]
pub struct PthreadCond {
    handle: AtomicUsize,
}
#[repr(C)]
pub struct PthreadRwLock {
    handle: AtomicUsize,
}
#[repr(C)]
pub struct PthreadRwLockAttr {
    pshared: c_int,
}
#[repr(C)]
pub struct PthreadMutexAttr {
    kind: c_int,
}
#[repr(C)]
pub struct PthreadCondAttr {
    clock: c_int,
}
impl PthreadMutex {
    pub const fn new() -> Self {
        Self {
            handle: AtomicUsize::new(0),
        }
    }
}
impl PthreadCond {
    pub const fn new() -> Self {
        Self {
            handle: AtomicUsize::new(0),
        }
    }
}
impl PthreadRwLock {
    pub const fn new() -> Self {
        Self {
            handle: AtomicUsize::new(0),
        }
    }
}

struct Registry<T> {
    entries: Vec<(usize, Arc<T>)>,
    next: usize,
}
impl<T> Registry<T> {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next: 1,
        }
    }
    fn insert(&mut self, state: T) -> Result<usize, c_int> {
        if self.next == DEAD {
            return Err(EAGAIN);
        }
        self.entries.try_reserve(1).map_err(|_| ENOMEM)?;
        let state = Arc::try_new(state).map_err(|_| ENOMEM)?;
        let handle = self.next;
        self.next += 1;
        self.entries.push((handle, state));
        Ok(handle)
    }
    fn get_or_insert(
        &mut self,
        handle: &AtomicUsize,
        create: impl FnOnce() -> T,
    ) -> Result<(usize, Arc<T>), c_int> {
        let mut id = handle.load(Ordering::Relaxed);
        if id == 0 {
            id = self.insert(create())?;
            handle.store(id, Ordering::Relaxed);
        }
        self.entries
            .iter()
            .find(|entry| entry.0 == id)
            .map(|entry| (id, Arc::clone(&entry.1)))
            .ok_or(EINVAL)
    }
    fn destroy(
        &mut self,
        handle: &AtomicUsize,
        require_unique: bool,
        busy: impl FnOnce(&T) -> bool,
    ) -> c_int {
        let id = handle.load(Ordering::Relaxed);
        if id == 0 {
            handle.store(DEAD, Ordering::Relaxed);
            return 0;
        }
        let Some(index) = self.entries.iter().position(|entry| entry.0 == id) else {
            return EINVAL;
        };
        let state = &self.entries[index].1;
        // A reference outside the registry covers an in-progress lock,
        // unlock, condition wait, or notification, preventing reclamation.
        if (require_unique && Arc::strong_count(state) != 1) || busy(state) {
            return EBUSY;
        }
        self.entries.swap_remove(index);
        handle.store(DEAD, Ordering::Relaxed);
        0
    }
}

struct MutexState {
    owner: usize,
    depth: usize,
}
struct MutexInner {
    kind: c_int,
    state: Mutex<MutexState>,
    changed: Condvar,
}
impl MutexInner {
    fn new(kind: c_int) -> Self {
        Self {
            kind,
            state: Mutex::new(MutexState { owner: 0, depth: 0 }),
            changed: Condvar::new(),
        }
    }
    fn acquire(&self, blocking: bool) -> c_int {
        let owner = crate::threading::current_id();
        let mut state = lock(&self.state);
        if state.owner == owner && self.kind == RECURSIVE {
            let Some(depth) = state.depth.checked_add(1) else {
                return EAGAIN;
            };
            state.depth = depth;
            return 0;
        }
        if state.owner != 0 && !blocking {
            return EBUSY;
        }
        if state.owner == owner && self.kind == ERRORCHECK {
            return EDEADLK;
        }
        while state.owner != 0 {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        state.owner = owner;
        state.depth = 1;
        0
    }
    fn release(&self, for_wait: bool) -> c_int {
        let mut state = lock(&self.state);
        if state.owner != crate::threading::current_id() {
            return EPERM;
        }
        // Waiting while recursively locked more than once cannot release
        // ownership for the producer. Reject instead of silently deadlocking.
        if for_wait && state.depth != 1 {
            return EINVAL;
        }
        state.depth -= 1;
        if state.depth == 0 {
            state.owner = 0;
            self.changed.notify_one();
        }
        0
    }
}
struct CondState {
    blocked: Vec<usize>,
    next_ticket: usize,
    bound_mutex: usize,
}
struct RwState {
    readers: Vec<(usize, usize)>,
    writer: usize,
    waiting_writers: usize,
}
struct RwInner {
    state: Mutex<RwState>,
    changed: Condvar,
}
impl RwInner {
    fn new() -> Self {
        Self {
            state: Mutex::new(RwState {
                readers: Vec::new(),
                writer: 0,
                waiting_writers: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn acquire_read(&self, blocking: bool) -> c_int {
        let owner = crate::threading::current_id();
        let mut state = lock(&self.state);
        if state.writer == owner {
            return EDEADLK;
        }
        while state.writer != 0
            || (state.waiting_writers != 0 && !state.readers.iter().any(|(id, _)| *id == owner))
        {
            if !blocking {
                return EBUSY;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if let Some((_, count)) = state.readers.iter_mut().find(|(id, _)| *id == owner) {
            let Some(next) = count.checked_add(1) else {
                return EAGAIN;
            };
            *count = next;
        } else {
            if state.readers.try_reserve(1).is_err() {
                return ENOMEM;
            }
            state.readers.push((owner, 1));
        }
        0
    }

    fn acquire_write(&self, blocking: bool) -> c_int {
        let owner = crate::threading::current_id();
        let mut state = lock(&self.state);
        if state.writer == owner || state.readers.iter().any(|(id, _)| *id == owner) {
            return EDEADLK;
        }
        if !blocking && (state.writer != 0 || !state.readers.is_empty()) {
            return EBUSY;
        }
        if state.writer != 0 || !state.readers.is_empty() {
            let Some(waiting) = state.waiting_writers.checked_add(1) else {
                return EAGAIN;
            };
            state.waiting_writers = waiting;
            while state.writer != 0 || !state.readers.is_empty() {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            state.waiting_writers -= 1;
        }
        state.writer = owner;
        0
    }

    fn release(&self) -> c_int {
        let owner = crate::threading::current_id();
        let mut state = lock(&self.state);
        if state.writer == owner {
            state.writer = 0;
            self.changed.notify_all();
            return 0;
        }
        let Some(index) = state.readers.iter().position(|(id, _)| *id == owner) else {
            return EPERM;
        };
        state.readers[index].1 -= 1;
        if state.readers[index].1 == 0 {
            state.readers.swap_remove(index);
        }
        if state.readers.is_empty() {
            self.changed.notify_all();
        }
        0
    }
}
impl CondState {
    fn remove(&mut self, ticket: usize) {
        if let Some(index) = self.blocked.iter().position(|value| *value == ticket) {
            self.blocked.swap_remove(index);
        }
        if self.blocked.is_empty() {
            self.bound_mutex = 0;
        }
    }
}
struct CondInner {
    clock: c_int,
    state: Mutex<CondState>,
    changed: Condvar,
}
impl CondInner {
    fn new(clock: c_int) -> Self {
        Self {
            clock,
            state: Mutex::new(CondState {
                blocked: Vec::new(),
                next_ticket: 0,
                bound_mutex: 0,
            }),
            changed: Condvar::new(),
        }
    }
}
static MUTEXES: Mutex<Registry<MutexInner>> = Mutex::new(Registry::new());
static CONDITIONS: Mutex<Registry<CondInner>> = Mutex::new(Registry::new());
static RWLOCKS: Mutex<Registry<RwInner>> = Mutex::new(Registry::new());

unsafe fn mutex(value: *mut PthreadMutex) -> Result<(usize, Arc<MutexInner>), c_int> {
    // SAFETY: callers must provide live, aligned pthread objects.
    let value = unsafe { value.as_ref() }.ok_or(EINVAL)?;
    lock(&MUTEXES).get_or_insert(&value.handle, || MutexInner::new(NORMAL))
}
unsafe fn condition(value: *mut PthreadCond) -> Result<(usize, Arc<CondInner>), c_int> {
    // SAFETY: callers must provide live, aligned pthread objects.
    let value = unsafe { value.as_ref() }.ok_or(EINVAL)?;
    lock(&CONDITIONS).get_or_insert(&value.handle, || CondInner::new(0))
}
unsafe fn rwlock(value: *mut PthreadRwLock) -> Result<(usize, Arc<RwInner>), c_int> {
    // SAFETY: callers must provide live, aligned pthread objects.
    let value = unsafe { value.as_ref() }.ok_or(EINVAL)?;
    lock(&RWLOCKS).get_or_insert(&value.handle, RwInner::new)
}
fn valid_kind(value: c_int) -> bool {
    matches!(value, NORMAL | RECURSIVE | ERRORCHECK)
}
fn valid_clock(value: c_int) -> bool {
    matches!(value, 0 | 1)
}

macro_rules! attr_functions {
    ($ty:ty, $field:ident, $valid:ident, $init:ident, $destroy:ident, $get:ident, $set:ident, $getpshared:ident, $setpshared:ident) => {
        /// # Safety
        /// The output must address writable attribute storage.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $init(attr: *mut $ty) -> c_int {
            let _errno = PreserveErrno::new();
            if attr.is_null() {
                return EINVAL;
            }
            // SAFETY: initialization may target uninitialized storage.
            unsafe { std::ptr::write(attr, <$ty>::new()) };
            0
        }
        /// # Safety
        /// attr must address an initialized attribute.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $destroy(attr: *mut $ty) -> c_int {
            let _errno = PreserveErrno::new();
            // SAFETY: required by the C API contract.
            let Some(attr) = (unsafe { attr.as_mut() }) else {
                return EINVAL;
            };
            if !$valid(attr.$field) {
                return EINVAL;
            }
            attr.$field = -1;
            0
        }
        /// # Safety
        /// attr must be initialized and output must be writable.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $get(attr: *const $ty, output: *mut c_int) -> c_int {
            let _errno = PreserveErrno::new();
            // SAFETY: required by the C API contract.
            let Some(attr) = (unsafe { attr.as_ref() }) else {
                return EINVAL;
            };
            if !$valid(attr.$field) || output.is_null() {
                return EINVAL;
            }
            // SAFETY: caller provides writable output.
            unsafe { *output = attr.$field };
            0
        }
        /// # Safety
        /// attr must address an initialized writable attribute.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $set(attr: *mut $ty, value: c_int) -> c_int {
            let _errno = PreserveErrno::new();
            // SAFETY: required by the C API contract.
            let Some(attr) = (unsafe { attr.as_mut() }) else {
                return EINVAL;
            };
            if !$valid(attr.$field) || !$valid(value) {
                return EINVAL;
            }
            attr.$field = value;
            0
        }
        /// # Safety
        /// attr must be initialized and output must be writable.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $getpshared(attr: *const $ty, output: *mut c_int) -> c_int {
            let _errno = PreserveErrno::new();
            // SAFETY: required by the C API contract.
            let Some(attr) = (unsafe { attr.as_ref() }) else {
                return EINVAL;
            };
            if !$valid(attr.$field) || output.is_null() {
                return EINVAL;
            }
            // SAFETY: caller provides writable output.
            unsafe { *output = 0 };
            0
        }
        /// # Safety
        /// attr must address an initialized attribute.
        #[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
        pub unsafe extern "C" fn $setpshared(attr: *mut $ty, value: c_int) -> c_int {
            let _errno = PreserveErrno::new();
            // SAFETY: required by the C API contract.
            let Some(attr) = (unsafe { attr.as_ref() }) else {
                return EINVAL;
            };
            if !$valid(attr.$field) {
                return EINVAL;
            }
            match value {
                0 => 0,
                1 => ENOTSUP,
                _ => EINVAL,
            }
        }
    };
}
impl PthreadMutexAttr {
    fn new() -> Self {
        Self { kind: NORMAL }
    }
}
impl PthreadCondAttr {
    fn new() -> Self {
        Self { clock: 0 }
    }
}
attr_functions!(
    PthreadMutexAttr,
    kind,
    valid_kind,
    pthread_mutexattr_init,
    pthread_mutexattr_destroy,
    pthread_mutexattr_gettype,
    pthread_mutexattr_settype,
    pthread_mutexattr_getpshared,
    pthread_mutexattr_setpshared
);
attr_functions!(
    PthreadCondAttr,
    clock,
    valid_clock,
    pthread_condattr_init,
    pthread_condattr_destroy,
    pthread_condattr_getclock,
    pthread_condattr_setclock,
    pthread_condattr_getpshared,
    pthread_condattr_setpshared
);

/// # Safety
/// value must address writable mutex storage; attr is null or initialized.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_mutex_init(
    value: *mut PthreadMutex,
    attr: *const PthreadMutexAttr,
) -> c_int {
    let _errno = PreserveErrno::new();
    if value.is_null() {
        return EINVAL;
    }
    // SAFETY: non-null attr points to an initialized attribute.
    let kind = unsafe { attr.as_ref() }.map_or(NORMAL, |attr| attr.kind);
    if !valid_kind(kind) {
        return EINVAL;
    }
    let mut registry = lock(&MUTEXES);
    let handle = match registry.insert(MutexInner::new(kind)) {
        Ok(handle) => handle,
        Err(error) => return error,
    };
    // SAFETY: init accepts uninitialized storage and has exclusive access.
    unsafe {
        std::ptr::write(
            value,
            PthreadMutex {
                handle: AtomicUsize::new(handle),
            },
        )
    };
    0
}
/// # Safety
/// value must address an initialized mutex.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_mutex_destroy(value: *mut PthreadMutex) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    let Some(value) = (unsafe { value.as_ref() }) else {
        return EINVAL;
    };
    lock(&MUTEXES).destroy(&value.handle, true, |inner| lock(&inner.state).owner != 0)
}
/// # Safety
/// value must address an initialized mutex.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_mutex_lock(value: *mut PthreadMutex) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { mutex(value) } {
        Ok((_, inner)) => inner.acquire(true),
        Err(error) => error,
    }
}
/// # Safety
/// value must address an initialized mutex.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_mutex_trylock(value: *mut PthreadMutex) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { mutex(value) } {
        Ok((_, inner)) => inner.acquire(false),
        Err(error) => error,
    }
}
/// # Safety
/// value must address an initialized mutex.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_mutex_unlock(value: *mut PthreadMutex) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { mutex(value) } {
        Ok((_, inner)) => inner.release(false),
        Err(error) => error,
    }
}

/// # Safety
/// value must address writable rwlock storage; attr is null or initialized.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_init(
    value: *mut PthreadRwLock,
    attr: *const PthreadRwLockAttr,
) -> c_int {
    let _errno = PreserveErrno::new();
    if value.is_null() {
        return EINVAL;
    }
    // SAFETY: a non-null attr is initialized by the C caller.
    if unsafe { attr.as_ref() }.is_some_and(|attr| attr.pshared != 0) {
        return ENOTSUP;
    }
    let handle = match lock(&RWLOCKS).insert(RwInner::new()) {
        Ok(handle) => handle,
        Err(error) => return error,
    };
    // SAFETY: init owns the uninitialized output storage.
    unsafe {
        std::ptr::write(
            value,
            PthreadRwLock {
                handle: AtomicUsize::new(handle),
            },
        )
    };
    0
}

/// # Safety
/// value must address an initialized rwlock.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_destroy(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    let Some(value) = (unsafe { value.as_ref() }) else {
        return EINVAL;
    };
    lock(&RWLOCKS).destroy(&value.handle, true, |inner| {
        let state = lock(&inner.state);
        state.writer != 0 || !state.readers.is_empty() || state.waiting_writers != 0
    })
}

/// # Safety
/// value must address an initialized rwlock.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_rdlock(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { rwlock(value) } {
        Ok((_, inner)) => inner.acquire_read(true),
        Err(error) => error,
    }
}

/// # Safety
/// value must address an initialized rwlock.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_tryrdlock(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { rwlock(value) } {
        Ok((_, inner)) => inner.acquire_read(false),
        Err(error) => error,
    }
}

/// # Safety
/// value must address an initialized rwlock.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_wrlock(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { rwlock(value) } {
        Ok((_, inner)) => inner.acquire_write(true),
        Err(error) => error,
    }
}

/// # Safety
/// value must address an initialized rwlock.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_trywrlock(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { rwlock(value) } {
        Ok((_, inner)) => inner.acquire_write(false),
        Err(error) => error,
    }
}

/// # Safety
/// value must address an initialized rwlock held by the calling thread.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_rwlock_unlock(value: *mut PthreadRwLock) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    match unsafe { rwlock(value) } {
        Ok((_, inner)) => inner.release(),
        Err(error) => error,
    }
}
/// # Safety
/// value must address writable condition storage; attr is null or initialized.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_init(
    value: *mut PthreadCond,
    attr: *const PthreadCondAttr,
) -> c_int {
    let _errno = PreserveErrno::new();
    if value.is_null() {
        return EINVAL;
    }
    // SAFETY: non-null attr points to an initialized attribute.
    let clock = unsafe { attr.as_ref() }.map_or(0, |attr| attr.clock);
    if !valid_clock(clock) {
        return EINVAL;
    }
    let mut registry = lock(&CONDITIONS);
    let handle = match registry.insert(CondInner::new(clock)) {
        Ok(handle) => handle,
        Err(error) => return error,
    };
    // SAFETY: init accepts uninitialized storage and has exclusive access.
    unsafe {
        std::ptr::write(
            value,
            PthreadCond {
                handle: AtomicUsize::new(handle),
            },
        )
    };
    0
}
/// # Safety
/// value must address an initialized condition variable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_destroy(value: *mut PthreadCond) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    let Some(value) = (unsafe { value.as_ref() }) else {
        return EINVAL;
    };
    // Unblocked waiters may still be reacquiring their user mutex. Their Arcs
    // retain the backend, but POSIX permits destroying/reusing this C object
    // after the last blocked waiter was signalled or broadcast.
    lock(&CONDITIONS).destroy(&value.handle, false, |inner| {
        !lock(&inner.state).blocked.is_empty()
    })
}

fn now(clock: c_int) -> Result<Duration, c_int> {
    #[cfg(target_os = "scarlet")]
    {
        let mut value = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: value is a live writable timespec.
        if unsafe { crate::runtime::clock_gettime(clock, &mut value) } != 0 {
            // SAFETY: the failure set this thread's errno cell.
            return Err(unsafe { *crate::__errno_location() });
        }
        Ok(Duration::new(value.tv_sec as u64, value.tv_nsec as u32))
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        if clock == 0 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| EINVAL)
        } else {
            static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            Ok(EPOCH.get_or_init(std::time::Instant::now).elapsed())
        }
    }
}

unsafe fn wait(
    value: *mut PthreadCond,
    mutex_value: *mut PthreadMutex,
    deadline: Option<Timespec>,
) -> c_int {
    if deadline.is_some_and(|time| !(0..1_000_000_000).contains(&time.tv_nsec)) {
        return EINVAL;
    }
    // SAFETY: required by the enclosing C API contract.
    let (_, cond) = match unsafe { condition(value) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    // SAFETY: required by the enclosing C API contract.
    let (mutex_id, mutex) = match unsafe { mutex(mutex_value) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut state = lock(&cond.state);
    if !state.blocked.is_empty() && state.bound_mutex != mutex_id {
        return EINVAL;
    }
    let Some(next_ticket) = state.next_ticket.checked_add(1) else {
        return EAGAIN;
    };
    if state.blocked.try_reserve(1).is_err() {
        return ENOMEM;
    }
    // Holding the condition lock across releasing the user mutex and entering
    // std::Condvar::wait closes the lost-wakeup window against notifications.
    let error = mutex.release(true);
    if error != 0 {
        return error;
    }
    let ticket = state.next_ticket;
    state.next_ticket = next_ticket;
    state.blocked.push(ticket);
    state.bound_mutex = mutex_id;
    let result = loop {
        // Notification removes tickets synchronously, so the dynamic binding
        // ends when the last waiter is unblocked, not when it runs again.
        if !state.blocked.contains(&ticket) {
            break 0;
        }
        if let Some(deadline) = deadline {
            let current = match now(cond.clock) {
                Ok(value) => value,
                Err(error) => break error,
            };
            let end = if deadline.tv_sec < 0 {
                Duration::ZERO
            } else {
                Duration::new(deadline.tv_sec as u64, deadline.tv_nsec as u32)
            };
            let Some(remaining) = end.checked_sub(current).filter(|value| !value.is_zero()) else {
                break ETIMEDOUT;
            };
            // Recheck realtime adjustments without losing the mutex/condition
            // atomicity. Intermediate timeout slices are not user wakeups.
            let interval = remaining.min(Duration::from_millis(100));
            let (guard, _) = cond
                .changed
                .wait_timeout(state, interval)
                .unwrap_or_else(|error| error.into_inner());
            state = guard;
        } else {
            state = cond
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    };
    state.remove(ticket);
    drop(state);
    // Only backend Arcs are used after unblocking. The caller may destroy or
    // reinitialize the original condition object while we reacquire its old
    // mutex; the new condition and its mutex binding remain independent.
    let acquired = mutex.acquire(true);
    if acquired != 0 { acquired } else { result }
}
/// # Safety
/// Both objects must be initialized; the caller must own mutex_value once.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_wait(
    value: *mut PthreadCond,
    mutex_value: *mut PthreadMutex,
) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: forwarded from the public API contract.
    unsafe { wait(value, mutex_value, None) }
}
/// # Safety
/// Objects must be initialized, the mutex owned once, and deadline readable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_timedwait(
    value: *mut PthreadCond,
    mutex_value: *mut PthreadMutex,
    deadline: *const Timespec,
) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: required by the C API contract.
    let Some(deadline) = (unsafe { deadline.as_ref() }) else {
        return EINVAL;
    };
    // SAFETY: forwarded from the public API contract.
    unsafe { wait(value, mutex_value, Some(*deadline)) }
}
unsafe fn notify(value: *mut PthreadCond, all: bool) -> c_int {
    // SAFETY: forwarded from the public API contract.
    let (_, cond) = match unsafe { condition(value) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut state = lock(&cond.state);
    if all {
        state.blocked.clear();
    } else {
        // std Condvar may wake any sleeper. Remove one selected ticket and
        // wake all internal sleepers; only that ticket can return to C.
        state.blocked.pop();
    }
    if state.blocked.is_empty() {
        state.bound_mutex = 0;
    }
    cond.changed.notify_all();
    0
}
/// # Safety
/// value must address an initialized condition variable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_signal(value: *mut PthreadCond) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: forwarded from the public API contract.
    unsafe { notify(value, false) }
}
/// # Safety
/// value must address an initialized condition variable.
#[cfg_attr(target_os = "scarlet", unsafe(no_mangle))]
pub unsafe extern "C" fn pthread_cond_broadcast(value: *mut PthreadCond) -> c_int {
    let _errno = PreserveErrno::new();
    // SAFETY: forwarded from the public API contract.
    unsafe { notify(value, true) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::sync::atomic::AtomicBool;

    fn mutex_ptr(value: &PthreadMutex) -> *mut PthreadMutex {
        ptr::from_ref(value).cast_mut()
    }
    fn cond_ptr(value: &PthreadCond) -> *mut PthreadCond {
        ptr::from_ref(value).cast_mut()
    }

    fn rwlock_ptr(value: &PthreadRwLock) -> *mut PthreadRwLock {
        ptr::from_ref(value).cast_mut()
    }

    #[test]
    fn rwlock_allows_concurrent_readers_and_blocks_writer_until_release() {
        let rwlock = Arc::new(PthreadRwLock::new());
        assert_eq!(unsafe { pthread_rwlock_rdlock(rwlock_ptr(&rwlock)) }, 0);
        let (read_ready_tx, read_ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let second = Arc::clone(&rwlock);
        let reader = std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_rwlock_rdlock(rwlock_ptr(&second)) }, 0);
            read_ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            assert_eq!(unsafe { pthread_rwlock_unlock(rwlock_ptr(&second)) }, 0);
        });
        read_ready_rx.recv().unwrap();
        assert_eq!(
            unsafe { pthread_rwlock_trywrlock(rwlock_ptr(&rwlock)) },
            EDEADLK
        );
        let third = Arc::clone(&rwlock);
        let (write_ready_tx, write_ready_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            assert_eq!(unsafe { pthread_rwlock_wrlock(rwlock_ptr(&third)) }, 0);
            write_ready_tx.send(()).unwrap();
            assert_eq!(unsafe { pthread_rwlock_unlock(rwlock_ptr(&third)) }, 0);
        });
        assert_eq!(
            unsafe { pthread_rwlock_destroy(rwlock_ptr(&rwlock)) },
            EBUSY
        );
        assert_eq!(unsafe { pthread_rwlock_unlock(rwlock_ptr(&rwlock)) }, 0);
        release_tx.send(()).unwrap();
        write_ready_rx.recv().unwrap();
        reader.join().unwrap();
        writer.join().unwrap();
        assert_eq!(unsafe { pthread_rwlock_destroy(rwlock_ptr(&rwlock)) }, 0);
    }

    #[test]
    fn recursive_errorcheck_and_errno() {
        unsafe {
            *crate::__errno_location() = 731;
            let mut attr = PthreadMutexAttr::new();
            assert_eq!(pthread_mutexattr_settype(&mut attr, 99), EINVAL);
            assert_eq!(pthread_mutexattr_setpshared(&mut attr, 1), ENOTSUP);
            assert_eq!(pthread_mutexattr_settype(&mut attr, RECURSIVE), 0);
            let mut value = PthreadMutex::new();
            assert_eq!(pthread_mutex_init(&mut value, &attr), 0);
            assert_eq!(pthread_mutex_lock(&mut value), 0);
            assert_eq!(pthread_mutex_trylock(&mut value), 0);
            assert_eq!(pthread_mutex_destroy(&mut value), EBUSY);
            assert_eq!(pthread_mutex_unlock(&mut value), 0);
            assert_eq!(pthread_mutex_destroy(&mut value), EBUSY);
            assert_eq!(pthread_mutex_unlock(&mut value), 0);
            assert_eq!(pthread_mutex_unlock(&mut value), EPERM);
            assert_eq!(pthread_mutex_destroy(&mut value), 0);
            assert_eq!(pthread_mutex_lock(&mut value), EINVAL);
            assert_eq!(pthread_mutexattr_settype(&mut attr, ERRORCHECK), 0);
            assert_eq!(pthread_mutex_init(&mut value, &attr), 0);
            assert_eq!(pthread_mutex_lock(&mut value), 0);
            assert_eq!(pthread_mutex_lock(&mut value), EDEADLK);
            assert_eq!(pthread_mutex_trylock(&mut value), EBUSY);
            assert_eq!(pthread_mutex_unlock(&mut value), 0);
            assert_eq!(pthread_mutex_destroy(&mut value), 0);
            assert_eq!(pthread_mutexattr_destroy(&mut attr), 0);
            assert_eq!(*crate::__errno_location(), 731);
        }
    }

    #[test]
    fn static_mutex_serializes_contention_and_rejects_wrong_owner() {
        let value = Arc::new(PthreadMutex::new());
        let count = Arc::new(AtomicUsize::new(0));
        unsafe {
            assert_eq!(pthread_mutex_lock(mutex_ptr(&value)), 0);
        }
        let clone = value.clone();
        std::thread::spawn(move || unsafe {
            *crate::__errno_location() = 932;
            assert_eq!(pthread_mutex_trylock(mutex_ptr(&clone)), EBUSY);
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&clone)), EPERM);
            assert_eq!(*crate::__errno_location(), 932);
        })
        .join()
        .unwrap();
        unsafe {
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&value)), 0);
        }
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let value = value.clone();
                let count = count.clone();
                std::thread::spawn(move || {
                    for _ in 0..2000 {
                        unsafe {
                            assert_eq!(pthread_mutex_lock(mutex_ptr(&value)), 0);
                        }
                        let previous = count.load(Ordering::Relaxed);
                        count.store(previous + 1, Ordering::Relaxed);
                        unsafe {
                            assert_eq!(pthread_mutex_unlock(mutex_ptr(&value)), 0);
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(count.load(Ordering::Relaxed), 16000);
        unsafe {
            assert_eq!(pthread_mutex_destroy(mutex_ptr(&value)), 0);
        }
    }

    #[test]
    fn condition_ping_pong_has_no_lost_wakeups() {
        let mutex = Arc::new(PthreadMutex::new());
        let cond = Arc::new(PthreadCond::new());
        let turn = Arc::new(AtomicBool::new(false));
        let (worker_mutex, worker_cond, worker_turn) = (mutex.clone(), cond.clone(), turn.clone());
        let worker = std::thread::spawn(move || unsafe {
            for _ in 0..1000 {
                assert_eq!(pthread_mutex_lock(mutex_ptr(&worker_mutex)), 0);
                while !worker_turn.load(Ordering::Relaxed) {
                    assert_eq!(
                        pthread_cond_wait(cond_ptr(&worker_cond), mutex_ptr(&worker_mutex)),
                        0
                    );
                }
                worker_turn.store(false, Ordering::Relaxed);
                assert_eq!(pthread_cond_signal(cond_ptr(&worker_cond)), 0);
                assert_eq!(pthread_mutex_unlock(mutex_ptr(&worker_mutex)), 0);
            }
        });
        unsafe {
            for _ in 0..1000 {
                assert_eq!(pthread_mutex_lock(mutex_ptr(&mutex)), 0);
                while turn.load(Ordering::Relaxed) {
                    assert_eq!(pthread_cond_wait(cond_ptr(&cond), mutex_ptr(&mutex)), 0);
                }
                turn.store(true, Ordering::Relaxed);
                assert_eq!(pthread_cond_signal(cond_ptr(&cond)), 0);
                assert_eq!(pthread_mutex_unlock(mutex_ptr(&mutex)), 0);
            }
        }
        worker.join().unwrap();
        unsafe {
            assert_eq!(pthread_cond_destroy(cond_ptr(&cond)), 0);
            assert_eq!(pthread_mutex_destroy(mutex_ptr(&mutex)), 0);
        }
    }

    #[test]
    fn timeout_reacquires_mutex_for_both_clocks() {
        unsafe {
            let mut mutex = PthreadMutex::new();
            let mut cond = PthreadCond::new();
            let mut attr = PthreadCondAttr::new();
            for clock in [0, 1] {
                assert_eq!(pthread_condattr_setclock(&mut attr, clock), 0);
                assert_eq!(pthread_cond_init(&mut cond, &attr), 0);
                assert_eq!(pthread_mutex_lock(&mut mutex), 0);
                let end = now(clock).unwrap() + Duration::from_millis(5);
                let deadline = Timespec {
                    tv_sec: end.as_secs() as i64,
                    tv_nsec: end.subsec_nanos() as _,
                };
                *crate::__errno_location() = 876;
                assert_eq!(
                    pthread_cond_timedwait(&mut cond, &mut mutex, &deadline),
                    ETIMEDOUT
                );
                assert_eq!(*crate::__errno_location(), 876);
                assert!(now(clock).unwrap() >= end);
                assert_eq!(pthread_mutex_trylock(&mut mutex), EBUSY);
                assert_eq!(pthread_mutex_unlock(&mut mutex), 0);
                assert_eq!(pthread_cond_destroy(&mut cond), 0);
            }
            assert_eq!(pthread_mutex_destroy(&mut mutex), 0);
        }
    }

    #[test]
    fn invalid_wait_preserves_ownership_and_condition_destroy_is_busy() {
        let mutex = Arc::new(PthreadMutex::new());
        let cond = Arc::new(PthreadCond::new());
        let ready = Arc::new(AtomicBool::new(false));
        let go = Arc::new(AtomicBool::new(false));
        let (wm, wc, wr, wg) = (mutex.clone(), cond.clone(), ready.clone(), go.clone());
        let worker = std::thread::spawn(move || unsafe {
            assert_eq!(pthread_mutex_lock(mutex_ptr(&wm)), 0);
            wr.store(true, Ordering::Release);
            while !wg.load(Ordering::Relaxed) {
                assert_eq!(pthread_cond_wait(cond_ptr(&wc), mutex_ptr(&wm)), 0);
            }
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&wm)), 0);
        });
        while !ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        unsafe {
            assert_eq!(pthread_mutex_lock(mutex_ptr(&mutex)), 0);
            assert_eq!(pthread_cond_destroy(cond_ptr(&cond)), EBUSY);
            assert_eq!(pthread_mutex_destroy(mutex_ptr(&mutex)), EBUSY);
            let invalid = Timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000,
            };
            assert_eq!(
                pthread_cond_timedwait(cond_ptr(&cond), mutex_ptr(&mutex), &invalid),
                EINVAL
            );
            assert_eq!(pthread_mutex_trylock(mutex_ptr(&mutex)), EBUSY);
            go.store(true, Ordering::Relaxed);
            assert_eq!(pthread_cond_broadcast(cond_ptr(&cond)), 0);
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&mutex)), 0);
        }
        worker.join().unwrap();
        unsafe {
            assert_eq!(pthread_cond_destroy(cond_ptr(&cond)), 0);
            assert_eq!(pthread_cond_signal(cond_ptr(&cond)), EINVAL);
            assert_eq!(pthread_mutex_destroy(mutex_ptr(&mutex)), 0);
        }
    }

    #[test]
    fn broadcast_ends_binding_before_old_mutex_reacquisition() {
        let old_mutex = Arc::new(PthreadMutex::new());
        let cond = Arc::new(PthreadCond::new());
        let ready = Arc::new(AtomicBool::new(false));
        let returned = Arc::new(AtomicBool::new(false));
        let (wm, wc, wr, wd) = (
            old_mutex.clone(),
            cond.clone(),
            ready.clone(),
            returned.clone(),
        );
        let worker = std::thread::spawn(move || unsafe {
            assert_eq!(pthread_mutex_lock(mutex_ptr(&wm)), 0);
            wr.store(true, Ordering::Release);
            assert_eq!(pthread_cond_wait(cond_ptr(&wc), mutex_ptr(&wm)), 0);
            wd.store(true, Ordering::Release);
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&wm)), 0);
        });
        while !ready.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        unsafe {
            // Acquiring this proves that W has entered wait, and keeping it
            // held prevents W from finishing after the broadcast.
            assert_eq!(pthread_mutex_lock(mutex_ptr(&old_mutex)), 0);
            assert_eq!(pthread_cond_broadcast(cond_ptr(&cond)), 0);
            let mut new_mutex = PthreadMutex::new();
            let expired = Timespec {
                tv_sec: -1,
                tv_nsec: 0,
            };
            assert_eq!(pthread_mutex_lock(&mut new_mutex), 0);
            assert_eq!(
                pthread_cond_timedwait(cond_ptr(&cond), &mut new_mutex, &expired),
                ETIMEDOUT
            );
            assert!(!returned.load(Ordering::Acquire));
            assert_eq!(pthread_cond_destroy(cond_ptr(&cond)), 0);
            assert_eq!(pthread_cond_init(cond_ptr(&cond), ptr::null()), 0);
            assert_eq!(
                pthread_cond_timedwait(cond_ptr(&cond), &mut new_mutex, &expired),
                ETIMEDOUT
            );
            assert_eq!(pthread_cond_destroy(cond_ptr(&cond)), 0);
            assert_eq!(pthread_mutex_unlock(&mut new_mutex), 0);
            assert_eq!(pthread_mutex_destroy(&mut new_mutex), 0);
            assert_eq!(pthread_mutex_unlock(mutex_ptr(&old_mutex)), 0);
        }
        worker.join().unwrap();
        assert!(returned.load(Ordering::Acquire));
        unsafe {
            assert_eq!(pthread_mutex_destroy(mutex_ptr(&old_mutex)), 0);
        }
    }

    #[test]
    fn in_flight_reference_blocks_destruction_and_stale_identity_stays_invalid() {
        unsafe {
            let mut value = PthreadMutex::new();
            let (identity, operation) = mutex(&mut value).unwrap();
            assert_eq!(pthread_mutex_destroy(&mut value), EBUSY);
            drop(operation);
            assert_eq!(pthread_mutex_destroy(&mut value), 0);
            assert_eq!(pthread_mutex_init(&mut value, ptr::null()), 0);
            assert_ne!(identity, value.handle.load(Ordering::Relaxed));
            let mut stale = PthreadMutex {
                handle: AtomicUsize::new(identity),
            };
            assert_eq!(pthread_mutex_lock(&mut stale), EINVAL);
            assert_eq!(pthread_mutex_destroy(&mut value), 0);
        }
    }
}
