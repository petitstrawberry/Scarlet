//! Run consecutive fallthrough instructions from one published fetch interval.
//! Enter a short native loop only after the guest takes its backward edge.
//! The unchanged branch displacement targets the same instruction in the slot.
use super::exclusive::Block;
use crate::platform_register::{self, RETURN};
use yaxpeax_arm::armv8::a64::Opcode;

pub(crate) fn taken_loop<'a>(
    branch: u32,
    pc: u64,
    target: u64,
    source: impl FnOnce(usize, usize) -> Option<&'a [u32]>,
) -> Option<Block> {
    let distance = pc.checked_sub(target)?;
    if distance > 31 * 4 || distance % 4 != 0 {
        return None;
    }
    if branch & 0xfc00_0000 != 0x1400_0000
        && branch & 0xff00_0010 != 0x5400_0000
        && branch & 0x7e00_0000 != 0x3400_0000
        && branch & 0x7e00_0000 != 0x3600_0000
    {
        return None;
    }
    let (last, body) = source(target as usize, (distance / 4 + 1) as usize)?.split_last()?;
    if *last != branch || !body.iter().copied().all(relocatable) {
        return None;
    }
    let mut words = body.to_vec();
    words.extend([branch, RETURN]);
    let pcs = (0..words.len()).map(|i| target + i as u64 * 4).collect();
    Block::finish(words, pcs)
}

/// Each slot PC has a canonical guest PC, including the first instruction
/// after the run. A signal or a fault can therefore stop at any instruction.
pub(crate) fn straight_line(pc: u64, source: &[u32]) -> Option<Block> {
    let length = source.iter().take(32).take_while(|&&w| relocatable(w)).count();
    if length < 2 {
        return None;
    }
    let mut words = source[..length].to_vec();
    words.push(RETURN);
    let pcs = (0..=length).map(|i| pc + i as u64 * 4).collect();
    Block::finish(words, pcs)
}

pub(super) fn relocatable(word: u32) -> bool {
    let Some(instruction) = platform_register::decode(word) else {
        return false;
    };
    // Position-independent integer operations and ordinary register-addressed
    // loads/stores have no hidden control transfer or syscall provenance.
    if matches!(word >> 25 & 15, 5 | 8 | 9 | 13) {
        return word & 0x1f00_0000 != 0x1000_0000;
    }
    use Opcode::*;
    word & 0x3b00_0000 != 0x1800_0000
        && matches!(
            instruction.opcode,
            LDR | LDRB
                | LDRH
                | LDRSB
                | LDRSH
                | LDRSW
                | LDUR
                | LDURB
                | LDURH
                | STR
                | STRB
                | STRH
                | STUR
                | STURB
                | STURH
                | LDP
                | STP
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_line_stops_before_pc_sensitive_words_and_preserves_fault_pcs() {
        let pc = 0x4004;
        let add = 0x9100_0400; // ADD X0,X0,#1
        let load = 0xf940_0041; // LDR X1,[X2]
        for stop in [
            0xd400_0001, // SVC
            0xd420_0000, // BRK
            0x1000_0000, // ADR
            0x5800_0000, // LDR literal
            0x1400_0000, // B
            0xc85f_7c20, // LDXR
        ] {
            let block = straight_line(pc, &[add, load, stop, add]).unwrap();
            assert_eq!(block.words, [add, load, RETURN]);
            assert_eq!(block.pcs, [pc, pc + 4, pc + 8]);
        }
        assert!(straight_line(pc, &[add]).is_none());
    }
}
