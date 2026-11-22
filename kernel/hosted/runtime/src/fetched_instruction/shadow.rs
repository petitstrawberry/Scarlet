//! One fixed entry per admitted guest word keeps internal branches in range.
//! PC-relative writes use local veneers with separate before/after stop PCs.
use crate::{platform_register, worker_memory::NativeSteps};
use std::{io, sync::OnceLock};

#[cfg(test)]
mod tests;

const WINDOW: usize = 1024;
pub(crate) const EXIT_IMMEDIATE: u32 = 0x5801;
pub(super) const EXIT: u32 = 0xd420_0000 | EXIT_IMMEDIATE << 5;

#[derive(Debug)]
struct Window {
    code: NativeSteps,
    count: usize,
    // Guest word indices before and after the literal load in each veneer.
    veneer_pcs: Box<[[u16; 2]]>,
}

// All indices refer to words in a single bounded shadow allocation.
fn branch(from: usize, to: usize) -> u32 {
    0x1400_0000 | ((to as u32).wrapping_sub(from as u32) & 0x03ff_ffff)
}

fn veneer(word: u32, pc: u64, index: usize, count: usize) -> Option<(u32, u64, usize)> {
    if word & 0x1f00_0000 == 0x1000_0000 {
        let rd = word & 31;
        if rd == 18 {
            return None;
        }
        let imm = super::signed(((word >> 5 & 0x7ffff) << 2) | (word >> 29 & 3), 21);
        let value = if word >> 31 == 0 {
            pc.wrapping_add_signed(imm)
        } else {
            (pc & !4095).wrapping_add_signed(imm << 12)
        };
        Some((rd, value, index + 1))
    } else if word & 0xfc00_0000 == 0x9400_0000 {
        let target = index.checked_add_signed(super::signed(word, 26) as isize)?;
        (target < count).then_some((30, pc + 4, target))
    } else {
        None
    }
}

impl Window {
    fn build(source: &[u32], guest_base: u64) -> io::Result<Self> {
        // Leave the fallthrough EXIT in place and align each 64-bit literal.
        let first_veneer = (source.len() + 1).next_multiple_of(2);
        let mut words = vec![EXIT; first_veneer];
        let mut veneer_pcs = Vec::new();
        for (i, &word) in source.iter().enumerate() {
            let pc = guest_base + i as u64 * 4;
            if let Some((rd, value, next)) = veneer(word, pc, i, source.len()) {
                let slot = words.len();
                words[i] = branch(i, slot);
                words.extend([
                    0x5800_0040 | rd,
                    branch(slot + 1, next),
                    value as u32,
                    (value >> 32) as u32,
                ]);
                veneer_pcs.push([i as u16, next as u16]);
                continue;
            }
            let native = if let Some(delta) = direct_branch(word) {
                let target = i.checked_add_signed(delta as isize);
                target.is_some_and(|target| {
                    if target >= source.len() {
                        return false;
                    }
                    // Let the existing taken-loop path rename x18 once for
                    // the entire loop instead of trapping once per iteration.
                    let short_mediated_loop = target < i
                        && i - target <= 31
                        && words[target..i].contains(&EXIT)
                        && source[target..i]
                            .iter()
                            .copied()
                            .all(super::loops::relocatable);
                    let tests_x18 = word & 0x7c00_0000 == 0x3400_0000 && word & 31 == 18;
                    !short_mediated_loop && !tests_x18
                })
            } else if matches!(word & !0x3e0, 0xd61f_0000 | 0xd65f_0000) {
                // BR and RET use the canonical value in the guest register.
                !matches!((word >> 5) & 31, 18 | 31)
            } else {
                (super::native_fallthrough(word) || word == 0xd503_201f)
                    && platform_register::plan(word, &mut Vec::new()).is_none()
            };
            if native {
                words[i] = word;
            }
        }
        Ok(Self {
            code: NativeSteps::seal(&words)?.expect("shadow contains an exit"),
            count: source.len(),
            veneer_pcs: veneer_pcs.into_boxed_slice(),
        })
    }

    fn original_offset(&self, address: u64) -> Option<usize> {
        let bytes = usize::try_from(address.checked_sub(self.code.address())?).ok()?;
        if !bytes.is_multiple_of(4) {
            return None;
        }
        let index = bytes / 4;
        if index <= self.count {
            return Some(bytes);
        }
        let index = index.checked_sub((self.count + 1).next_multiple_of(2))?;
        // Only the LDR and B have PCs. The two literal words are data.
        let guest = self.veneer_pcs.get(index / 4)?.get(index % 4)?;
        Some(usize::from(*guest) * 4)
    }

    fn entry(&self, index: usize) -> Option<u64> {
        let address = self.code.address() + index as u64 * 4;
        // Only called for an admitted index into this immutable RX allocation.
        (unsafe { (address as *const u32).read() } != EXIT).then_some(address)
    }
}

fn direct_branch(word: u32) -> Option<i64> {
    if word & 0xfc00_0000 == 0x1400_0000 {
        Some(super::signed(word, 26))
    } else if word & 0xff00_0010 == 0x5400_0000 || word & 0x7e00_0000 == 0x3400_0000 {
        Some(super::signed(word >> 5, 19))
    } else if word & 0x7e00_0000 == 0x3600_0000 {
        Some(super::signed(word >> 5, 14))
    } else {
        None
    }
}

#[derive(Debug)]
pub(crate) struct Shadows(Box<[OnceLock<Window>]>);

impl Shadows {
    pub(crate) fn new(count: usize) -> Self {
        Self(
            (0..count.div_ceil(WINDOW))
                .map(|_| OnceLock::new())
                .collect(),
        )
    }

    pub(crate) fn entry(
        &self,
        source: &[u32],
        index: usize,
        guest_base: u64,
    ) -> io::Result<Option<u64>> {
        let tile = index / WINDOW;
        let cell = &self.0[tile];
        if cell.get().is_none() {
            let source = &source[tile * WINDOW..];
            // Concurrent first entry is harmless. The losing RX allocation
            // drops before any thread can have entered it.
            let _ = cell.set(Window::build(
                &source[..source.len().min(WINDOW)],
                guest_base + (tile * WINDOW * 4) as u64,
            )?);
        }
        Ok(self.cached_entry(index))
    }

    pub(crate) fn cached_entry(&self, index: usize) -> Option<u64> {
        self.0[index / WINDOW].get()?.entry(index % WINDOW)
    }

    pub(crate) fn original_offset(&self, address: u64) -> Option<usize> {
        for (tile, cell) in self.0.iter().enumerate() {
            if let Some(window) = cell.get()
                && let Some(offset) = window.original_offset(address)
            {
                return Some(tile * WINDOW * 4 + offset);
            }
        }
        None
    }
}
