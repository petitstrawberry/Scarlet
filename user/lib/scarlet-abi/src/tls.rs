//! Native userspace TLS mapping shared by the CRT, Rust std and C ABI.
//!
//! This is a runtime layout, not ELF TLS. A matching CRT initializes it before
//! constructors or user code; a thread creator initializes it before clone.
//! The initial mapping lives until process exit, and spawned-thread mappings
//! are released by the existing native thread-exit cleanup protocol.

/// Identifies the namespace-based TLS layout with an allocation-free errno.
pub const NATIVE_TLS_MAGIC: u32 = 0x5343_5401;
pub const NATIVE_TLS_SLOT_COUNT: usize = 1024;
pub const NATIVE_TLS_CLEANUP_OFFSET: usize = NATIVE_TLS_SLOT_COUNT * size_of::<usize>();
pub const NATIVE_TLS_MAPPING_SIZE: usize = NATIVE_TLS_CLEANUP_OFFSET + 4096;

/// Header at the architectural thread pointer (TPIDR_EL0 or tp).
///
/// Fields are owned by this thread, including when accessed by another DSO.
/// Namespace nodes remain private to the matching Rust TLS implementation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NativeTlsHeader {
    pub namespace_head: usize,
    pub magic: u32,
    pub errno: i32,
}

impl NativeTlsHeader {
    pub const INITIAL: Self = Self {
        namespace_head: 0,
        magic: NATIVE_TLS_MAGIC,
        errno: 0,
    };
}

const _: () = {
    assert!(core::mem::offset_of!(NativeTlsHeader, namespace_head) == 0);
    assert!(core::mem::offset_of!(NativeTlsHeader, magic) == size_of::<usize>());
    assert!(core::mem::offset_of!(NativeTlsHeader, errno) == size_of::<usize>() + 4);
    assert!(size_of::<NativeTlsHeader>() <= NATIVE_TLS_CLEANUP_OFFSET);
};
