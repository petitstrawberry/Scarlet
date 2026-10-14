//! Native userspace TLS mapping shared by the CRT, Rust std and C ABI.
//!
//! A matching CRT initializes the header before constructors or user code;
//! a thread creator initializes it before clone. On AArch64, static executable
//! ELF TLS follows the header and is initialized by SetTls/Clone.
//! The initial mapping lives until process exit, and spawned-thread mappings
//! are released by the existing native thread-exit cleanup protocol.

/// Identifies the namespace-based TLS layout with an allocation-free errno.
pub const NATIVE_TLS_MAGIC: u32 = 0x5343_5401;
pub const NATIVE_TLS_SLOT_COUNT: usize = 1024;
pub const NATIVE_TLS_CLEANUP_OFFSET: usize = NATIVE_TLS_SLOT_COUNT * size_of::<usize>();
pub const NATIVE_TLS_MAPPING_SIZE: usize = NATIVE_TLS_CLEANUP_OFFSET + 4096;

/// Offset of an AArch64 executable's static ELF TLS block after its 16-byte TCB.
/// Dynamic DSO TLS and nonzero template alignment residues are not supported.
pub const fn aarch64_elf_tls_offset(alignment: usize, memory_size: usize) -> Option<usize> {
    if alignment == 0 || !alignment.is_power_of_two() || alignment > 4096 {
        return None;
    }
    let offset = match 16usize.checked_next_multiple_of(alignment) {
        Some(offset) => offset,
        None => return None,
    };
    match offset.checked_add(memory_size) {
        Some(end) if end <= NATIVE_TLS_CLEANUP_OFFSET => Some(offset),
        _ => None,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_tls_never_overlaps_header_or_thread_cleanup() {
        assert_eq!(aarch64_elf_tls_offset(1, 8176), Some(16));
        assert_eq!(aarch64_elf_tls_offset(32, 8160), Some(32));
        assert_eq!(aarch64_elf_tls_offset(4096, 4096), Some(4096));
        for (alignment, size) in [
            (0, 0),
            (3, 8),
            (8192, 0),
            (16, 8177),
            (4096, 4097),
            (16, usize::MAX),
        ] {
            assert_eq!(aarch64_elf_tls_offset(alignment, size), None);
        }
    }
}
