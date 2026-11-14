//! Linux ABI configuration vocabulary, not Scarlet kernel build settings.
//!
//! Only implemented ABI facilities are listed. In particular, this does not
//! claim Linux Binder, SELinux, or other host implementation details.

macro_rules! configuration {
    ($architecture:literal) => {
        concat!(
            "# Scarlet Linux ABI compatibility facilities\n",
            "# This is not the native kernel build configuration.\n",
            $architecture,
            "CONFIG_BINFMT_ELF=y\n",
            "CONFIG_EPOLL=y\n",
            "CONFIG_EVENTFD=y\n",
            "CONFIG_SIGNALFD=y\n",
            "CONFIG_TIMERFD=y\n",
            "CONFIG_FUTEX=y\n",
            "CONFIG_INOTIFY_USER=y\n",
        )
        .as_bytes()
    };
}

#[cfg(target_arch = "aarch64")]
const TEXT: &[u8] = configuration!("CONFIG_ARM64=y\nCONFIG_64BIT=y\n");
#[cfg(target_arch = "riscv64")]
const TEXT: &[u8] = configuration!("CONFIG_RISCV=y\nCONFIG_64BIT=y\n");
#[cfg(target_arch = "riscv32")]
const TEXT: &[u8] = configuration!("CONFIG_RISCV=y\nCONFIG_32BIT=y\n");
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "riscv32"
)))]
const TEXT: &[u8] = configuration!("");

const fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    let mut i = 0;
    while i < bytes.len() {
        crc ^= bytes[i] as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        i += 1;
    }
    !crc
}

/// One RFC 1952 member with a stored DEFLATE block. Generated at compile time;
/// no compressor, mutable global state, or runtime compression is needed.
const fn gzip<const N: usize>(text: &[u8]) -> [u8; N] {
    assert!(text.len() <= u16::MAX as usize && N == text.len() + 23);
    let mut bytes = [0; N];
    bytes[0] = 0x1f;
    bytes[1] = 0x8b;
    bytes[2] = 8;
    bytes[9] = 255;
    bytes[10] = 1; // Final, uncompressed DEFLATE block.
    let len = text.len() as u16;
    let length = len.to_le_bytes();
    let inverse = (!len).to_le_bytes();
    bytes[11] = length[0];
    bytes[12] = length[1];
    bytes[13] = inverse[0];
    bytes[14] = inverse[1];
    let mut i = 0;
    while i < text.len() {
        bytes[15 + i] = text[i];
        i += 1;
    }
    let crc = crc32(text).to_le_bytes();
    let size = (text.len() as u32).to_le_bytes();
    i = 0;
    while i < 4 {
        bytes[N - 8 + i] = crc[i];
        bytes[N - 4 + i] = size[i];
        i += 1;
    }
    bytes
}

pub(super) const GZIP: &[u8] = &gzip::<{ TEXT.len() + 23 }>(TEXT);

#[cfg(test)]
mod tests {
    #[test_case]
    fn stored_gzip_matches_an_independent_crc_fixture() {
        // CRC-32/ISO-HDLC check value for ASCII "123456789".
        assert_eq!(super::crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            super::gzip::<32>(b"123456789"),
            [
                31, 139, 8, 0, 0, 0, 0, 0, 0, 255, 1, 9, 0, 246, 255, 49, 50, 51, 52, 53, 54, 55,
                56, 57, 38, 57, 244, 203, 9, 0, 0, 0,
            ]
        );
    }

    #[test_case]
    fn compatibility_config_does_not_claim_foreign_kernel_implementations() {
        let text = core::str::from_utf8(super::TEXT).unwrap();
        assert!(text.contains("CONFIG_EPOLL=y\n"));
        assert!(!text.contains("CONFIG_SECURITY_SELINUX="));
        assert!(!text.contains("CONFIG_ANDROID_BINDER_IPC="));
        assert_eq!(&super::GZIP[15..super::GZIP.len() - 8], super::TEXT);
    }
}
