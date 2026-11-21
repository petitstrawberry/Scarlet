//! SGFX mapped-session messages carried by the Scarlet GPU queue. Every
//! integer is a little-endian u64. Attachment tokens are context-local.
#![no_std]
extern crate alloc;
use alloc::vec::Vec;
pub const BACKEND: &[u8] = b"scarlet-hosted";
pub const DIALECT: u64 = 1;
pub const SNAPSHOT: u64 = 1;
pub const MAP: u64 = 2;
pub const UNMAP: u64 = 3;
pub const EXECUTE: u64 = 4;
pub const START: u64 = 5;
pub const APPEND: u64 = 6;
pub const FINISH: u64 = 7;
pub const VALIDATE: u64 = 8;
pub const RELEASE_BUFFER: u64 = 9;
pub const RELEASE_TEXTURE: u64 = 10;
pub const RELEASE_GROUP: u64 = 11;
/// Top-level synchronous commands. The last word names a writable attached transfer image.
pub const READ_BUFFER: u64 = 12;
pub const READ_TEXTURE: u64 = 13;
pub const RELEASE_SHADER: u64 = 14;
pub const RELEASE_COMPUTE_PIPELINE: u64 = 15;
pub const RELEASE_RENDER_PIPELINE: u64 = 16;
pub const MAX_READBACK: usize = 64 << 20;
pub const MAX_MESSAGE: usize = 96 << 20;

/// Visit exactly the upload spans in a pinned SGFX v2 command recording.
/// The length must match the schema *before* replacing either word. The
/// in-process decoder otherwise consumes a pointer before checking trailing
/// fields. No other opcode in this ABI contains pointers.
pub fn uploads(
    mut words: &mut [u64],
    count: usize,
    mut visit: impl FnMut(&mut u64, usize) -> Result<(), ()>,
) -> Result<(), ()> {
    for _ in 0..count {
        let header = *words.first().ok_or(())?;
        let length = (header >> 32) as usize;
        if length == 0 || length > words.len() {
            return Err(());
        }
        let (cmd, rest) = words.split_at_mut(length);
        words = rest;
        let expected = match header as u32 {
            1 => 6,  // WriteBuffer
            2 => 12, // WriteTexture
            26 => 5, // SetPushConstants
            3..=25 | 27..=29 => continue,
            _ => return Err(()),
        };
        if length != expected {
            return Err(());
        }
        let len = usize::try_from(cmd[length - 1]).map_err(|_| ())?;
        visit(&mut cmd[length - 2], len)?;
    }
    if words.is_empty() { Ok(()) } else { Err(()) }
}

/// Snapshot a borrowed SGFX batch into its final transport allocation.
///
/// # Safety
/// Every nonempty upload span in the recording must be readable and remain
/// immutable until this function returns, as required by the SGFX plugin ABI.
pub unsafe fn pack_recording(table: u64, count: usize, record: &[u64]) -> Result<Vec<u64>, ()> {
    let header = [EXECUTE, table, count as u64, record.len() as u64];
    let prefix = header.len().checked_add(record.len()).ok_or(())?;
    if prefix > MAX_MESSAGE / 8 {
        return Err(());
    }
    let mut words = Vec::with_capacity(prefix);
    words.extend_from_slice(&header);
    words.extend_from_slice(record);
    let mut total = 0usize;
    uploads(&mut words[header.len()..], count, |addr, len| {
        if len == 0 {
            *addr = 0;
        } else if *addr == 0 {
            return Err(());
        }
        total = total.checked_add(len).ok_or(())?;
        Ok(())
    })?;
    if total == 0 {
        return Ok(words);
    }
    let extra = total.div_ceil(8);
    if extra > MAX_MESSAGE / 8 - prefix {
        return Err(());
    }
    words.reserve_exact(extra);
    let dst = words.spare_capacity_mut().as_mut_ptr().cast::<u8>();
    let mut cursor = 0;
    uploads(&mut words[header.len()..], count, |addr, len| {
        if len != 0 {
            // The caller owns the source. dst is disjoint spare capacity and
            // the first pass bounded the sum of all lengths before reservation.
            unsafe { core::ptr::copy_nonoverlapping(*addr as *const u8, dst.add(cursor), len) };
        }
        *addr = cursor as u64;
        cursor += len;
        Ok(())
    })?;
    unsafe {
        core::ptr::write_bytes(dst.add(total), 0, extra * 8 - total);
        words.set_len(prefix + extra);
    }
    Ok(words)
}

pub fn as_bytes(words: &[u64]) -> &[u8] {
    // Both endpoints are little-endian AArch64. u64 has no padding.
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast(), core::mem::size_of_val(words)) }
}

#[cfg(test)]
mod tests {
    use super::uploads;

    #[test]
    fn packs_unaligned_spans_and_zeroes_only_final_padding() {
        let a = [1u8, 2, 3];
        let b = [4u8, 5, 6, 7, 8, 9];
        let header = (5u64 << 32) | 26;
        let record = [
            header,
            0,
            0,
            a.as_ptr() as u64,
            3,
            header,
            0,
            0,
            0,
            0,
            header,
            0,
            0,
            b.as_ptr() as u64,
            6,
        ];
        let packet = unsafe { super::pack_recording(42, 3, &record) }.unwrap();
        assert_eq!(&packet[..4], &[super::EXECUTE, 42, 3, 15]);
        assert_eq!([packet[7], packet[12], packet[17]], [0, 3, 3]);
        assert_eq!(
            super::as_bytes(&packet[19..]),
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(unsafe { super::pack_recording(42, 1, &[header, 0, 0, 0, 3]) }.is_err());
    }

    #[test]
    fn relocates_only_the_span_of_each_upload() {
        for (opcode, length) in [(1, 6), (2, 12), (26, 5)] {
            let mut command = [0; 12];
            command[0] = (length as u64) << 32 | opcode;
            command[length - 2] = 24;
            command[length - 1] = 8;
            let mut visits = 0;
            uploads(&mut command[..length], 1, |offset, size| {
                assert_eq!((*offset, size), (24, 8));
                *offset = 64;
                visits += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(visits, 1);
            assert_eq!(command[length - 2], 64);
        }
    }

    #[test]
    fn rejects_incomplete_uploads_before_visiting_a_span() {
        for header in [
            0,
            (5 << 32) | 1,
            (12 << 32) | 2,
            (6 << 32) | 26,
            (6 << 32) | 30,
        ] {
            let mut command = [header, 0, 0, 0, 0, 0];
            assert!(uploads(&mut command, 1, |_, _| panic!("unexpected upload")).is_err());
        }
    }
}
