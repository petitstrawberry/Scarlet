#![no_std]
extern crate alloc;
mod engine;
pub mod profile;
pub use engine::{Adapter, Engine, Error, WPA2_PSK_CCMP_IE};
#[cfg(all(feature = "native", target_os = "scarlet"))]
pub mod native;
unsafe extern "C" {
    fn swpa_pmk(pass: *const u8, ssid: *const u8, len: usize, pmk: *mut u8) -> i32;
}
/// Derive the WPA2 PSK using upstream PBKDF2-HMAC-SHA1.
pub fn derive_pmk(passphrase: &str, ssid: &[u8]) -> Result<[u8; 32], ()> {
    if !(8..=63).contains(&passphrase.len())
        || passphrase.as_bytes().contains(&0)
        || ssid.is_empty()
        || ssid.len() > 32
    {
        return Err(());
    }
    let mut pass = alloc::vec::Vec::from(passphrase.as_bytes());
    pass.push(0);
    let mut result = [0; 32];
    let status = unsafe {
        swpa_pmk(
            pass.as_ptr(),
            ssid.as_ptr(),
            ssid.len(),
            result.as_mut_ptr(),
        )
    };
    use zeroize::Zeroize;
    pass.zeroize();
    if status == 0 {
        Ok(result)
    } else {
        result.zeroize();
        Err(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn upstream_pbkdf2_known_vector() {
        let p = super::derive_pmk("password", b"IEEE").unwrap();
        assert_eq!(
            p,
            [
                0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a,
                0x5f, 0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e,
                0x97, 0x10, 0xa1, 0x2e
            ]
        );
    }
}

#[cfg(test)]
mod engine_smoke {
    use super::*;
    struct Driver;
    impl Adapter for Driver {
        fn random(&mut self, b: &mut [u8]) -> Result<(), Error> {
            b.fill(0x55);
            Ok(())
        }
        fn now_ns(&self) -> u64 {
            0
        }
        fn send(&mut self, _: [u8; 6], _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
        fn key(&mut self, _: u32, _: [u8; 6], _: u32, _: &[u8], _: &[u8]) -> Result<(), Error> {
            Ok(())
        }
        fn set_authorized(&mut self, _: bool) -> Result<(), Error> {
            Ok(())
        }
        fn abort(&mut self) {}
    }
    #[test]
    fn upstream_state_constructs() {
        let engine = Engine::new(
            Driver,
            [0x52, 0x54, 0, 0, 0, 1],
            [2, 0, 0, 0, 0, 1],
            b"IEEE",
            "password",
            &WPA2_PSK_CCMP_IE,
            5_000_000_000,
        )
        .unwrap();
        assert!(!engine.authorized());
    }
}

// C-owned allocations use the same allocator as the Rust caller. The C port
// retains exact size/alignment metadata and wipes each allocation before free.
#[unsafe(no_mangle)]
unsafe extern "C" fn swpa_rust_alloc(size: usize, align: usize) -> *mut core::ffi::c_void {
    let Ok(layout) = alloc::alloc::Layout::from_size_align(size, align) else {
        return core::ptr::null_mut();
    };
    unsafe { alloc::alloc::alloc(layout).cast() }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn swpa_rust_free(ptr: *mut core::ffi::c_void, size: usize, align: usize) {
    if ptr.is_null() {
        return;
    }
    if let Ok(layout) = alloc::alloc::Layout::from_size_align(size, align) {
        unsafe {
            alloc::alloc::dealloc(ptr.cast(), layout);
        }
    }
}

#[cfg(all(feature = "native", target_os = "scarlet"))]
#[unsafe(no_mangle)]
extern "C" fn swpa_native_abort() -> ! {
    // Exit closes Native control handles, which revoke data and retire keys.
    unsafe {
        scarlet_sys::syscall1(scarlet_sys::Syscall::ExitGroup, 127);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Authenticated unicast PTK protection metadata in the Native EAPOL wire ABI.
#[cfg(feature = "native")]
pub use scarlet_abi::wireless::EAPOL_PROTECTED;
#[cfg(not(feature = "native"))]
pub const EAPOL_PROTECTED: u32 = 1;
