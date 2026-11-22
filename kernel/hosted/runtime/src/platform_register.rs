//! Keep the AArch64 guest's x18 independent of Darwin's reserved register.
//! Ordinary register operations run on the CPU with an unused operand register.
//! PC-sensitive operations use the original guest address.
use yaxpeax_arch::{Decoder, U8Reader};
use yaxpeax_arm::armv8::a64::{InstDecoder, Instruction, Opcode, Operand, SizeCode};

const FIELDS: [u32; 4] = [0, 5, 10, 16];
pub(super) const RETURN: u32 = 0xd420_0000 | (0x5800 << 5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NativeLoop {
        slot: usize,
        entry: u8,
        exit: u8,
        scratch: u8,
    },
    Native {
        slot: usize,
        scratch: u8,
        literal: Option<i64>,
        paired: bool,
    },
    Move {
        source: u8,
        destination: u8,
        wide: bool,
    },
    Address {
        offset: i64,
        page: bool,
    },
    Jump {
        link: bool,
    },
    Test {
        mask: u64,
        nonzero: bool,
        offset: i64,
    },
    Unsupported,
}

pub(super) fn decode(word: u32) -> Option<Instruction> {
    InstDecoder::default()
        .decode(&mut U8Reader::new(&word.to_le_bytes()))
        .ok()
}

fn registers(operand: &mut Operand, mut visit: impl FnMut(&mut u16)) {
    use Operand::*;
    match operand {
        Register(_, n)
        | RegisterOrSP(_, n)
        | RegShift(_, _, _, n)
        | RegPreIndex(n, ..)
        | RegPostIndex(n, _) => visit(n),
        RegRegOffset(base, index, ..) | RegPostIndexReg(base, index) => {
            visit(base);
            visit(index);
        }
        _ => {}
    }
}

/// Register-only and ordinary memory operations that can borrow LR for the
/// virtual x18 in a linked run. Exclude explicit and implicit LR operands.
pub(super) fn linked_word(word: u32) -> Option<u32> {
    if !FIELDS.iter().any(|s| matches!((word >> s) & 31, 18 | 30)) {
        return Some(word);
    }
    let mut insn = decode(word)?;
    let mut uses_lr = false;
    for operand in &mut insn.operands {
        if matches!(operand, Operand::RegisterPair(..)) {
            return None;
        }
        registers(operand, |r| uses_lr |= *r == 30);
    }
    if uses_lr {
        return None;
    }
    rename_x18(word, insn, 30)
}

pub(super) fn rename_x18(word: u32, mut insn: Instruction, scratch: u8) -> Option<u32> {
    for op in &mut insn.operands {
        registers(op, |r| {
            if *r == 18 {
                *r = scratch.into();
            }
        });
    }
    let fields: Vec<_> = FIELDS
        .into_iter()
        .filter(|s| word >> s & 31 == 18)
        .collect();
    if decode(word) == Some(insn) {
        return Some(word);
    }
    for subset in 1usize..(1 << fields.len()) {
        let mut candidate = word;
        for (i, shift) in fields.iter().enumerate() {
            if subset & (1 << i) != 0 {
                candidate ^= (18 ^ u32::from(scratch)) << shift;
            }
        }
        if decode(candidate) == Some(insn) {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn plan(word: u32, steps: &mut Vec<u32>) -> Option<Action> {
    if !FIELDS.iter().any(|shift| (word >> shift) & 31 == 18) {
        return None;
    }
    // ORR Wd,WZR,Wm or Xd,XZR,Xm (unshifted register MOV). Immediate
    // and shifted forms still use the general decoder and native execution.
    if word & 0x7fe0_ffe0 == 0x2a00_03e0 {
        let source = ((word >> 16) & 31) as u8;
        let destination = (word & 31) as u8;
        return (source == 18 || destination == 18).then_some(Action::Move {
            source,
            destination,
            wide: word >> 31 != 0,
        });
    }
    if crate::exclusive::is_pair_compare_exchange(word) {
        let compare = (word >> 16) & 31;
        let value = word & 31;
        let base = (word >> 5) & 31;
        if compare != 18 && value != 18 && base != 18 {
            return None;
        }
        // The high register in each pair is implicit in the instruction.
        // Keep x18 and x19 together, including when either names the base.
        let used = (3u32 << compare) | (3u32 << value) | (1u32 << base);
        let scratch = (0..16).step_by(2).find(|index| used & (3 << index) == 0)?;
        let rename = |reg| match reg {
            18 => scratch,
            19 => scratch + 1,
            _ => reg,
        };
        let native = (word & !0x001f_03ff)
            | (rename(compare) << 16) | (rename(base) << 5) | rename(value);
        let slot = steps.len();
        steps.extend([native, RETURN]);
        return Some(Action::Native {
            slot,
            scratch: scratch as u8,
            paired: true,
            literal: None,
        });
    }
    let mut insn = decode(word)?;
    let mut used = 0u32;
    for op in &mut insn.operands {
        if let Operand::RegisterPair(_, n) = *op {
            if n == 18 {
                return Some(Action::Unsupported);
            }
            used |= 3 << n;
        }
        registers(op, |r| used |= 1 << *r);
    }
    if used & (1 << 18) == 0 {
        return None;
    }
    use Opcode::*;
    use Operand::*;
    let action = match (insn.opcode, insn.operands) {
        (ADR | ADRP, [_, PCOffset(offset), ..]) => Action::Address {
            offset,
            page: insn.opcode == ADRP,
        },
        (BR | BLR | RET, _) => Action::Jump {
            link: insn.opcode == BLR,
        },
        (CBZ | CBNZ, [Register(size, _), PCOffset(offset), ..]) => Action::Test {
            mask: if size == SizeCode::W {
                u32::MAX.into()
            } else {
                u64::MAX
            },
            nonzero: insn.opcode == CBNZ,
            offset,
        },
        (TBZ | TBNZ, [_, Imm16(bit), PCOffset(offset), _]) if bit < 64 => Action::Test {
            mask: 1 << bit,
            nonzero: insn.opcode == TBNZ,
            offset,
        },
        (LDR | LDRSW, [Register(size, 18), PCOffset(offset), ..]) => {
            // ldr{sw} W0/X0,[X0]: the step uses the guest literal address;
            // its load (including faults) is performed by the processor.
            let native = match (insn.opcode, size) {
                (LDRSW, _) => 0xb980_0000,
                (_, SizeCode::W) => 0xb940_0000,
                (_, SizeCode::X) => 0xf940_0000,
            };
            let slot = steps.len();
            steps.extend([native, RETURN]);
            Action::Native {
                slot,
                scratch: 0,
                literal: Some(offset),
                paired: false,
            }
        }
        // A relocated instruction must fall through to RETURN. Exceptions must
        // see the guest PC, and exclusive sequences cannot cross a signal.
        _ if insn.operands.iter().any(|op| matches!(op, PCOffset(_)))
            || word & 0x1c00_0000 == 0x1400_0000
            || (word & 0x3f80_0000 == 0x0800_0000) =>
        {
            Action::Unsupported
        }
        _ => {
            let scratch = (!used).trailing_zeros() as u8;
            // Compare complete decoded operands so an immediate or a SIMD
            // register numbered 18 is never renamed with an integer operand.
            let renamed = rename_x18(word, insn, scratch);
            match renamed {
                Some(native) => {
                    let slot = steps.len();
                    steps.extend([native, RETURN]);
                    Action::Native {
                        slot,
                        scratch,
                        literal: None,
                        paired: false,
                    }
                }
                None => Action::Unsupported,
            }
        }
    };
    Some(action)
}
