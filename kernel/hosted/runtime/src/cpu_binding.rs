//! Logical CPU identity follows the Darwin thread, including across a
//! migrated Scarlet stack. A native TLS address cached in a suspended frame
//! would continue to refer to that frame's previous Darwin thread.
use std::{ffi::c_void, ptr, sync::LazyLock};

static KEY: LazyLock<libc::pthread_key_t> = LazyLock::new(|| {
    let mut key = 0;
    assert_eq!(unsafe { libc::pthread_key_create(&mut key, None) }, 0);
    key
});

fn state() -> usize {
    unsafe { libc::pthread_getspecific(*KEY).addr() }
}

fn set_state(value: usize) {
    let value = ptr::without_provenance_mut::<c_void>(value);
    assert_eq!(unsafe { libc::pthread_setspecific(*KEY, value) }, 0);
}

pub fn bind(id: usize) {
    set_state((id + 1) << 1);
}

pub fn unbind() {
    set_state(0);
}

pub fn current() -> Option<usize> {
    (state() >> 1).checked_sub(1)
}

pub fn interrupts_enabled() -> bool {
    state() & 1 != 0
}

pub fn set_interrupts(enabled: bool) -> bool {
    let old = state();
    set_state((old & !1) | usize::from(enabled));
    old & 1 != 0
}
