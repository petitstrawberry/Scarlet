//! Lower only the instruction at an admitted guest PC. The canonical mapping
//! never executes on Darwin. Data loads, including literal loads, use guest VAs.
mod exclusive;
pub(crate) mod linked;
mod loops;
pub(crate) mod shadow;
pub(super) use loops::{straight_line, taken_loop};

use crate::{
    platform_register::{self, Action},
    worker_context::Registers,
    worker_memory::{NativeSteps, SYSCALL_BREAKPOINT, svc_immediate},
};
use std::{
    collections::{HashMap, VecDeque},
    io,
};

// Cache only position-independent native words. Each entry gets its guest PC,
// syscall provenance and canonical bytes from the currently admitted mapping.
// A worker thread updates its own slots only while outside guest execution.
const SLOTS_PER_PAGE_GROUP: usize = 32;
const MAX_PAGE_GROUPS: usize = 32;

#[derive(Default)]
pub(super) struct Slots {
    addresses: HashMap<Box<[u32]>, u64>,
    pending: Vec<(Box<[u32]>, NativeSteps)>,
    groups: VecDeque<(Vec<Box<[u32]>>, NativeSteps)>,
}

impl Slots {
    pub fn address(&mut self, words: &[u32]) -> io::Result<u64> {
        if let Some(&address) = self.addresses.get(words) {
            return Ok(address);
        }
        if self.pending.len() == SLOTS_PER_PAGE_GROUP {
            self.pack()?;
        }
        let slot = NativeSteps::seal(words)?.expect("Empty native slot");
        let address = slot.address();
        self.addresses.insert(words.into(), address);
        self.pending.push((words.into(), slot));
        Ok(address)
    }

    // A freshly encountered block can execute immediately on its own sealed
    // page. Once enough have accumulated, pack them into a single allocation
    // so recently used code can survive beyond the first few dozen blocks.
    fn pack(&mut self) -> io::Result<()> {
        let mut words = Vec::new();
        for (block, _) in &self.pending {
            words.extend_from_slice(block);
        }
        let page = NativeSteps::seal(&words)?.expect("Empty group of native slots");
        if self.groups.len() == MAX_PAGE_GROUPS {
            let (blocks, _old_page) = self.groups.pop_front().unwrap();
            for block in blocks {
                self.addresses.remove(block.as_ref());
            }
        }
        let mut next = page.address();
        let mut blocks = Vec::with_capacity(self.pending.len());
        for (block, _) in self.pending.drain(..) {
            *self.addresses.get_mut(block.as_ref()).unwrap() = next;
            next += size_of_val(block.as_ref()) as u64;
            blocks.push(block);
        }
        self.groups.push_back((blocks, page));
        Ok(())
    }
}

pub(super) enum Fetch {
    Advanced,
    Undefined,
    Block(exclusive::Block),
    Native {
        word: u32,
        // Scratch is seeded just before guest entry and restored at every stop.
        scratch: Option<(usize, u64)>,
        writes_x18: bool,
        paired: bool,
        svc: Option<u16>,
    },
}

fn read(regs: &Registers, register: usize) -> u64 {
    regs.x.get(register).copied().unwrap_or(0)
}

fn write(regs: &mut Registers, register: usize, value: u64) {
    if let Some(r) = regs.x.get_mut(register) {
        *r = value;
    }
}

pub(super) fn branch_offset(word: u32) -> i64 {
    signed(word, 26) * 4
}

fn signed(value: u32, bits: u32) -> i64 {
    i64::from(((value << (32 - bits)) as i32) >> (32 - bits))
}

pub(crate) fn condition(cond: u32, nzcv: u32) -> bool {
    let n = nzcv & (1 << 31) != 0;
    let z = nzcv & (1 << 30) != 0;
    let c = nzcv & (1 << 29) != 0;
    let v = nzcv & (1 << 28) != 0;
    let set = match cond >> 1 {
        0 => z,
        1 => c,
        2 => n,
        3 => v,
        4 => c && !z,
        5 => n == v,
        6 => n == v && !z,
        _ => return true,
    };
    set != (cond & 1 != 0)
}

fn literal(word: u32, regs: &Registers) -> Fetch {
    let rt = word & 31;
    let vector = word & (1 << 26) != 0;
    let op = match (vector, word >> 30) {
        (false, 0) => 0xb940_0000, // LDR W
        (false, 1) => 0xf940_0000, // LDR X
        (false, 2) => 0xb980_0000, // LDRSW
        (false, 3) => 0xf980_0000, // PRFM
        (true, 0) => 0xbd40_0000,  // LDR S
        (true, 1) => 0xfd40_0000,  // LDR D
        (true, 2) => 0x3dc0_0000,  // LDR Q
        _ => return Fetch::Undefined,
    };
    let base = u32::from(rt == 0);
    let writes_x18 = !vector && rt == 18 && word >> 30 != 3;
    let target = if writes_x18 { base } else { rt };
    let address = regs.pc.wrapping_add_signed(signed(word >> 5, 19) * 4);
    Fetch::Native {
        word: op | base << 5 | target,
        scratch: Some((base as usize, address)),
        writes_x18,
        paired: false,
        svc: None,
    }
}

/// Instructions the worker may copy to another PC without changing operands.
/// Keep exclusive sequences together in the exclusive-window path.
fn native_fallthrough(word: u32) -> bool {
    matches!(word >> 25 & 15, 4..=9 | 12..=15)
        && word & 0x1f00_0000 != 0x1000_0000 // ADR and ADRP
        && word & 0x3b00_0000 != 0x1800_0000 // literal loads
        && (word & 0x3f80_0000 != 0x0800_0000
            || crate::exclusive::is_pair_compare_exchange(word))
        && platform_register::decode(word).is_some()
}

pub(super) fn lower(
    word: u32,
    regs: &mut Registers,
    tls: &mut u64,
    fetch: impl Fn(u64) -> Option<u32>,
) -> Fetch {
    let pc = regs.pc;
    let next = pc.wrapping_add(4);
    let rt = (word & 31) as usize;
    if let Some(kind) = exclusive::kind(word) {
        return exclusive::Block::plan(pc, word, kind, fetch)
            .map_or(Fetch::Undefined, Fetch::Block);
    }
    // A fetched memset sees the same DCZID_EL0 as native text. DC ZVA
    // runs at the canonical data VA so its fault reaches the checked backend.
    if matches!(word & !31, 0xd53b_00e0 | 0xd50b_7420) {
        let rename = rt == 18;
        return Fetch::Native {
            word: if rename { word & !31 } else { word },
            scratch: rename.then_some((0, regs.x[18])),
            writes_x18: rename && word & !31 == 0xd53b_00e0,
            paired: false,
            svc: None,
        };
    }
    if let Some(immediate) = svc_immediate(word) {
        return Fetch::Native {
            word: SYSCALL_BREAKPOINT,
            scratch: None,
            writes_x18: false,
            paired: false,
            svc: Some(immediate),
        };
    }
    if word & 0x3b00_0000 == 0x1800_0000 {
        return literal(word, regs);
    }
    if word & 0x1f00_0000 == 0x1000_0000 {
        // ADR, ADRP
        let offset = signed(((word >> 5 & 0x7ffff) << 2) | (word >> 29 & 3), 21);
        let value = if word >> 31 != 0 {
            (pc & !4095).wrapping_add_signed(offset * 4096)
        } else {
            pc.wrapping_add_signed(offset)
        };
        write(regs, rt, value);
    } else if word & 0x7c00_0000 == 0x1400_0000 {
        // B, BL
        if word >> 31 != 0 {
            regs.x[30] = next;
        }
        regs.pc = pc.wrapping_add_signed(signed(word, 26) * 4);
        return Fetch::Advanced;
    } else if word & 0xff00_0010 == 0x5400_0000 {
        // B.cond
        regs.pc = if condition(word & 15, regs.nzcv) {
            pc.wrapping_add_signed(signed(word >> 5, 19) * 4)
        } else {
            next
        };
        return Fetch::Advanced;
    } else if word & 0x7e00_0000 == 0x3400_0000 || word & 0x7e00_0000 == 0x3600_0000 {
        let value = read(regs, rt);
        let (nonzero, bits) = if word & (1 << 25) == 0 {
            // CBZ, CBNZ
            (
                if word >> 31 == 0 {
                    value as u32 != 0
                } else {
                    value != 0
                },
                19,
            )
        } else {
            // TBZ, TBNZ
            let bit = (word >> 31) * 32 + (word >> 19 & 31);
            (value & (1 << bit) != 0, 14)
        };
        regs.pc = if nonzero == (word & (1 << 24) != 0) {
            pc.wrapping_add_signed(signed(word >> 5, bits) * 4)
        } else {
            next
        };
        return Fetch::Advanced;
    } else if matches!(word & !0x3e0, 0xd61f_0000 | 0xd63f_0000 | 0xd65f_0000) {
        let target = read(regs, (word >> 5 & 31) as usize); // BR, BLR, RET
        if word & !0x3e0 == 0xd63f_0000 {
            regs.x[30] = next;
        }
        regs.pc = target;
        return Fetch::Advanced;
    } else if word & !31 == 0xd53b_d040 {
        write(regs, rt, *tls);
    } else if word & !31 == 0xd51b_d040 {
        *tls = read(regs, rt);
    } else {
        // Relocate fall-through instructions only. A signal between exclusive
        // operations would lose their monitor. Unknown system/control forms
        // may use implicit operands or change PC and cannot run in a slot.
        let brk = word & 0xffe0_001f == 0xd420_0000;
        // User cache maintenance for newly written instructions and the
        // corresponding memory/instruction barriers. Retain the guest VA.
        let maintenance = matches!(word & !31, 0xd50b_7b20 | 0xd50b_7520)
            || matches!(
                word & !0xf00,
                0xd503_309f | 0xd503_30bf | 0xd503_30df | 0xd503_305f
            );
        if !native_fallthrough(word) && !brk && !maintenance && word != 0xd503_201f {
            return Fetch::Undefined;
        }
        let mut words = Vec::new();
        match platform_register::plan(word, &mut words) {
            None => {
                return Fetch::Native {
                    word,
                    scratch: None,
                    writes_x18: false,
                    paired: false,
                    svc: None,
                };
            }
            Some(Action::Native {
                slot,
                scratch,
                literal: None,
                paired,
            }) => {
                return Fetch::Native {
                    word: words[slot],
                    scratch: Some((usize::from(scratch), regs.x[18])),
                    writes_x18: true,
                    paired,
                    svc: None,
                };
            }
            Some(Action::Move {
                source,
                destination,
                wide,
            }) => {
                let value = read(regs, source as usize);
                write(
                    regs,
                    destination as usize,
                    if wide { value } else { u64::from(value as u32) },
                );
            }
            _ => return Fetch::Undefined,
        }
    }
    regs.pc = next;
    Fetch::Advanced
}

#[cfg(test)]
mod slot_tests {
    use super::{SLOTS_PER_PAGE_GROUP, Slots};
    use crate::platform_register::RETURN;

    #[test]
    fn packed_blocks_retain_their_words_and_individual_entry_points() {
        let mut slots = Slots::default();
        let block = |n: usize| [0x9100_0000 | (n as u32) << 10, RETURN];
        for n in 0..=SLOTS_PER_PAGE_GROUP {
            slots.address(&block(n)).unwrap();
        }
        let base = slots.address(&block(0)).unwrap();
        for n in 0..SLOTS_PER_PAGE_GROUP {
            let pc = slots.address(&block(n)).unwrap();
            assert_eq!(pc, base + n as u64 * 8);
            // The thread is parked and owns these sealed readable pages.
            assert_eq!(unsafe { (pc as *const [u32; 2]).read() }, block(n));
        }
    }
}
