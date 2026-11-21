//! Keep exclusive loads and stores in one native run. A stop between them
//! loses the local monitor. A resumed STXR then takes its normal failure path.
use crate::platform_register::{self, RETURN};
use yaxpeax_arm::armv8::a64::Opcode;

pub(super) enum Kind {
    Load,
    Store,
}

pub(super) fn kind(word: u32) -> Option<Kind> {
    if word & 0x3f80_0000 != 0x0800_0000 {
        return None;
    }
    use Opcode::*;
    match platform_register::decode(word)?.opcode {
        LDXR | LDXRB | LDXRH | LDAXR | LDAXRB | LDAXRH | LDXP | LDAXP => Some(Kind::Load),
        STXR | STXRB | STXRH | STLXR | STLXRB | STLXRH | STXP | STLXP => Some(Kind::Store),
        _ => None,
    }
}

pub(crate) struct Block {
    pub words: Vec<u32>,
    // One guest PC per native instruction, including each exit breakpoint.
    pub pcs: Vec<u64>,
    pub scratch: Option<usize>,
}

impl Block {
    pub(super) fn finish(mut words: Vec<u32>, pcs: Vec<u64>) -> Option<Self> {
        // Keep a single scratch for the entire native window so preemption
        // can reconstruct x18 at any instruction, including the store.
        let mut scratch = None;
        if words
            .iter()
            .any(|&w| platform_register::plan(w, &mut Vec::new()).is_some())
        {
            let mut used = 1u32 << 18;
            for &word in &words {
                // Conservatively include fields which might name a register.
                for shift in [0, 5, 10, 16] {
                    used |= 1 << (word >> shift & 31);
                }
                let insn = platform_register::decode(word)?;
                if insn
                    .operands
                    .iter()
                    .any(|op| matches!(op, yaxpeax_arm::armv8::a64::Operand::RegisterPair(..)))
                {
                    return None;
                }
            }
            let index = (!used).trailing_zeros();
            if index >= 31 {
                return None;
            }
            for word in &mut words {
                *word = platform_register::rename_x18(
                    *word,
                    platform_register::decode(*word)?,
                    index as u8,
                )?;
            }
            scratch = Some(index as usize);
        }
        Some(Self {
            words,
            pcs,
            scratch,
        })
    }

    pub(super) fn plan(
        pc: u64,
        word: u32,
        instruction: Kind,
        fetch: impl Fn(u64) -> Option<u32>,
    ) -> Option<Self> {
        if matches!(instruction, Kind::Store) {
            // Entry in the middle, including a resume after preemption. Do
            // not accidentally reuse an unrelated native exclusive monitor.
            return Self::finish(vec![0xd503_3f5f, word, RETURN], vec![pc, pc, pc + 4]);
        }
        Self::load_window(pc, word, fetch).or_else(|| {
            // A standalone load (for example LDXR followed by CLREX) still
            // reads normally. A later store resumes with a cleared monitor.
            Self::finish(vec![word, 0xd503_3f5f, RETURN], vec![pc, pc + 4, pc + 4])
        })
    }

    fn load_window(pc: u64, word: u32, fetch: impl Fn(u64) -> Option<u32>) -> Option<Self> {
        let mut words = vec![word];
        let mut pcs = vec![pc];
        let mut exits: Vec<(usize, u64, u32)> = Vec::new();
        // AArch64 recommends at most 128 bytes between LDXR and STXR.
        for i in 1..crate::exclusive::MAX_WORDS as u64 {
            let here = pc.checked_add(i * 4)?;
            let next = fetch(here)?;
            pcs.push(here);
            words.push(next);
            if matches!(kind(next), Some(Kind::Store)) {
                words.push(RETURN);
                pcs.push(here + 4);
                for (index, target, mask) in exits {
                    if target < here + 4 {
                        return None;
                    }
                    // Keep the condition and redirect the exit to a dedicated
                    // breakpoint. Both fallthrough and taken exits retain NZCV.
                    words[index] = (words[index] & !mask) | (((words.len() - index) as u32) << 5);
                    words.push(RETURN);
                    pcs.push(target);
                }
                return Self::finish(words, pcs);
            }
            let bits = match next {
                w if w & 0xff00_0010 == 0x5400_0000 || w & 0x7e00_0000 == 0x3400_0000 => 19,
                w if w & 0x7e00_0000 == 0x3600_0000 => 14,
                _ => 0,
            };
            if bits != 0 {
                let target = here.wrapping_add_signed(super::signed(next >> 5, bits) * 4);
                exits.push((words.len() - 1, target, ((1u32 << bits) - 1) << 5));
                continue;
            }
            // Only position-independent, register-only operations may occur
            // before the store. Never translate across a call or literal pool.
            if !matches!(next >> 25 & 15, 5 | 8 | 9 | 13)
                || next & 0x1f00_0000 == 0x1000_0000
                || platform_register::decode(next).is_none()
            {
                return None;
            }
        }
        None
    }
}
