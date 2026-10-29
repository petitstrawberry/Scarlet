//! Bounded ELF identification; it does not replace the loader's validation.
//! Unmarked executables require explicit ABI selection, even from Android.

use crate::{
    fs::SeekFrom,
    object::KernelObject,
    task::elf_loader::{ElfHeader, ProgramHeader},
};

pub(super) fn is_android_aarch64(object: &KernelObject) -> bool {
    let Some(file) = object.as_file() else {
        return false;
    };
    scan(|offset, buffer| {
        file.seek(SeekFrom::Start(offset)).ok()?;
        let mut done = 0;
        while done < buffer.len() {
            let n = file.read(&mut buffer[done..]).ok()?;
            if n == 0 {
                return None;
            }
            done += n;
        }
        Some(())
    })
    .unwrap_or(false)
}

fn scan(mut read: impl FnMut(u64, &mut [u8]) -> Option<()>) -> Option<bool> {
    let mut raw = [0; 64];
    read(0, &mut raw)?;
    let header = ElfHeader::parse(&raw).ok()?;
    if raw[7] != 0
        || header.ei_class != 2
        || header.ei_data != 1
        || header.e_machine != 183
        || !matches!(header.e_type, 2 | 3)
        || header.e_phentsize != 56
        || header.e_phnum == 0
        || header.e_phnum > 128
    {
        return None;
    }
    let mut android_note = false;
    let mut interpreter = None;
    let mut note_budget = 64 * 1024u64;
    for index in 0..header.e_phnum {
        let mut raw = [0; 56];
        let offset = header.e_phoff.checked_add(u64::from(index) * 56)?;
        read(offset, &mut raw)?;
        let ph = ProgramHeader::parse(&raw, 2, true).ok()?;
        let end = ph.p_offset.checked_add(ph.p_filesz)?;
        match ph.p_type {
            3 => {
                // PT_INTERP
                if interpreter.is_some() || ph.p_filesz == 0 || ph.p_filesz > 256 {
                    return None;
                }
                let mut path = [0; 256];
                let path = &mut path[..ph.p_filesz as usize];
                read(ph.p_offset, path)?;
                interpreter = Some(matches!(
                    path as &[u8],
                    b"/system/bin/linker64\0"
                        | b"/system/bin/bootstrap/linker64\0"
                        | b"/apex/com.android.runtime/bin/linker64\0"
                ));
            }
            4 => {
                // PT_NOTE; names/descriptors are each padded to four bytes.
                note_budget = note_budget.checked_sub(ph.p_filesz)?;
                let mut at = ph.p_offset;
                while at < end {
                    if end - at < 12 {
                        return None;
                    }
                    let mut note = [0; 12];
                    read(at, &mut note)?;
                    let namesz = u32::from_le_bytes(note[..4].try_into().ok()?) as u64;
                    let descsz = u32::from_le_bytes(note[4..8].try_into().ok()?) as u64;
                    let kind = u32::from_le_bytes(note[8..].try_into().ok()?);
                    let name = at.checked_add(12)?;
                    let desc = name.checked_add((namesz + 3) & !3)?;
                    at = desc.checked_add((descsz + 3) & !3)?;
                    if at > end {
                        return None;
                    }
                    if namesz == 8 && kind == 1 && descsz >= 4 {
                        let mut owner = [0; 8];
                        read(name, &mut owner)?;
                        // Read the API level too: a header with a truncated
                        // descriptor must not identify an otherwise empty file.
                        let mut api = [0; 4];
                        read(desc, &mut api)?;
                        android_note |= &owner == b"Android\0";
                    }
                }
            }
            _ => {}
        }
    }
    // A foreign interpreter takes precedence over an unrelated Android note.
    Some(interpreter.unwrap_or(android_note))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{vec, vec::Vec};
    fn elf(kind: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 120];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&kind.to_le_bytes());
        bytes[72..80].copy_from_slice(&120u64.to_le_bytes());
        bytes[96..104].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }
    fn identified(bytes: &[u8]) -> bool {
        scan(|offset, output| {
            let start = usize::try_from(offset).ok()?;
            output.copy_from_slice(bytes.get(start..start.checked_add(output.len())?)?);
            Some(())
        })
        .unwrap_or(false)
    }
    #[test_case]
    fn detects_android_interpreter_but_not_generic_linux_or_wrong_architecture() {
        assert!(identified(&elf(3, b"/system/bin/linker64\0")));
        assert!(identified(&elf(3, b"/system/bin/bootstrap/linker64\0")));
        assert!(!identified(&elf(3, b"/system/bin/bootstrap/linker64-other\0")));
        assert!(identified(&elf(
            3,
            b"/apex/com.android.runtime/bin/linker64\0"
        )));
        assert!(!identified(&elf(3, b"/lib/ld-linux-aarch64.so.1\0")));
        assert!(!identified(&elf(3, b"/system/bin/linker64")));
        assert!(!identified(&elf(1, b"Android\0")));
        let mut wrong_arch = elf(3, b"/system/bin/linker64\0");
        wrong_arch[18] = 62;
        assert!(!identified(&wrong_arch));
    }
    #[test_case]
    fn detects_static_android_notes_and_rejects_truncated_or_overflowing_headers() {
        let mut note = Vec::new();
        for word in [8u32, 4, 1] {
            note.extend_from_slice(&word.to_le_bytes());
        }
        note.extend_from_slice(b"Android\0");
        note.extend_from_slice(&33u32.to_le_bytes());
        let mut bytes = elf(4, &note);
        assert!(identified(&bytes));
        for length in 0..bytes.len() {
            assert!(!identified(&bytes[..length]));
        }
        bytes[120..124].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!identified(&bytes));
        bytes[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(!identified(&bytes));
    }
}
