//! Scalar AArch64 exclusive accesses on subpages requiring mediated data access.
//! The worker supplies instructions from its admitted mapping and the register
//! state before the window. No software reservation survives a guest stop.
use crate::worker_context::Registers;

pub const MAX_WORDS: usize = 32;

/// CASP shares an encoding group with LL/SC, but its memory operation is a
/// single instruction. Both operand pairs must start at an even register.
pub fn is_pair_compare_exchange(word: u32) -> bool {
    word & 0xbfa1_7c01 == 0x0820_7c00
}

#[derive(Clone, Copy, Debug)]
pub struct Access {
    pub load: bool,
    pub ordered: bool,
    pub width: usize,
    pub base: usize,
    pub value: usize,
    pub status: usize,
}

impl Access {
    pub fn decode(word: u32) -> Option<Self> {
        let load = word & 0x3fff_7c00 == 0x085f_7c00;
        if !load && word & 0x3fe0_7c00 != 0x0800_7c00 {
            return None;
        }
        let access = Self {
            load,
            ordered: word & 0x8000 != 0,
            width: 1 << (word >> 30),
            base: (word >> 5 & 31) as usize,
            value: (word & 31) as usize,
            status: (word >> 16 & 31) as usize,
        };
        // Constrained-unpredictable store status overlap.
        if !load
            && access.status != 31
            && (access.status == access.value || access.status == access.base)
        {
            return None;
        }
        Some(access)
    }

    pub fn address(self, r: &Registers) -> u64 {
        r.x.get(self.base).copied().unwrap_or(r.sp)
    }
}

#[derive(Debug)]
pub struct Window {
    pub initial: Registers,
    pub words: Vec<u32>,
}

#[derive(Clone, Copy)]
enum Operation {
    Access(Access),
    Arithmetic {
        sub: bool,
        flags: bool,
        immediate: bool,
    },
    Logical,
    Select,
    Branch {
        displacement: i64,
        condition: u32,
    },
    RegisterBranch {
        displacement: i64,
        mask: u64,
        nonzero: bool,
    },
}

fn signed(value: u32, bits: u32) -> i64 {
    i64::from(((value << (32 - bits)) as i32) >> (32 - bits))
}

fn decode(w: u32) -> Option<Operation> {
    if let Some(a) = Access::decode(w) {
        return Some(Operation::Access(a));
    }
    if w & 0x1f20_0000 == 0x0b00_0000 || w & 0x1f80_0000 == 0x1100_0000 {
        let immediate = w & (1 << 24) != 0 && w & (1 << 25) == 0;
        if !immediate && (w >> 22 & 3 == 3 || w >> 31 == 0 && w >> 10 & 63 >= 32) {
            return None;
        }
        return Some(Operation::Arithmetic {
            sub: w & (1 << 30) != 0,
            flags: w & (1 << 29) != 0,
            immediate,
        });
    }
    if w & 0x1f00_0000 == 0x0a00_0000 && (w >> 31 != 0 || w >> 10 & 63 < 32) {
        return Some(Operation::Logical);
    }
    if w & 0x3fe0_0800 == 0x1a80_0000 {
        return Some(Operation::Select);
    }
    if w & 0xff00_0010 == 0x5400_0000 {
        return Some(Operation::Branch {
            displacement: signed(w >> 5, 19) * 4,
            condition: w & 15,
        });
    }
    if w & 0x7e00_0000 == 0x3400_0000 || w & 0x7e00_0000 == 0x3600_0000 {
        let (mask, bits) = if w & (1 << 25) == 0 {
            (
                if w >> 31 == 0 {
                    u32::MAX.into()
                } else {
                    u64::MAX
                },
                19,
            )
        } else {
            (1 << ((w >> 31) * 32 + (w >> 19 & 31)), 14)
        };
        return Some(Operation::RegisterBranch {
            displacement: signed(w >> 5, bits) * 4,
            mask,
            nonzero: w & (1 << 24) != 0,
        });
    }
    None
}

pub(crate) fn retry_instruction(word: u32) -> bool {
    decode(word).is_some() || word & 0xfc00_0000 == 0x1400_0000
}

pub enum Outcome {
    Branch,
    Store(Access),
}

impl Window {
    pub fn capture(
        initial: Registers,
        mut fetch: impl FnMut(usize) -> Option<u32>,
    ) -> Option<Self> {
        let pc = usize::try_from(initial.pc).ok()?;
        if pc % 4 != 0 {
            return None;
        }
        let word = fetch(pc)?;
        let load = Access::decode(word)?;
        if !load.load {
            return None;
        }
        let mut words = vec![word];
        let mut first_exit = usize::MAX;
        for i in 1..MAX_WORDS {
            let address = pc.checked_add(i * 4)?;
            if address >= first_exit {
                return None;
            }
            let word = fetch(address)?;
            words.push(word);
            match decode(word)? {
                Operation::Access(store) if !store.load && store.width == load.width => {
                    return Some(Self { initial, words });
                }
                Operation::Access(_) => return None,
                Operation::Branch { displacement, .. }
                | Operation::RegisterBranch { displacement, .. } => {
                    first_exit =
                        first_exit.min(address.checked_add_signed(displacement.try_into().ok()?)?);
                }
                _ => {}
            }
        }
        None
    }

    /// Evaluate only register effects. The final store has not run, so its
    /// status is untouched and a genuine denial reports this exact guest PC.
    pub fn evaluate(&self, observed: u64, regs: &mut Registers) -> Option<Outcome> {
        let load = Access::decode(*self.words.first()?)?;
        set(regs, load.value, observed);
        for &w in self.words.get(1..)? {
            regs.pc = regs.pc.checked_add(4)?;
            let wide = w >> 31 != 0;
            let mask = if wide { u64::MAX } else { u64::from(u32::MAX) };
            let rn = (w >> 5 & 31) as usize;
            let rd = (w & 31) as usize;
            let rm = (w >> 16 & 31) as usize;
            let a = get(regs, rn) & mask;
            let b = get(regs, rm) & mask;
            let bits = if wide { 64 } else { 32 };
            let n = w >> 10 & 63;
            let shift=|| match w>>22&3 {
                0=>b<<n, 1=>b>>n, 2 if wide=>((b as i64)>>n) as u64,
                2=>((b as i32)>>n) as u32 as u64,
                _ if wide=>b.rotate_right(n), _=>(b as u32).rotate_right(n) as u64,
            } & mask;
            match decode(w)? {
                Operation::Access(store) if !store.load => return Some(Outcome::Store(store)),
                Operation::Access(_) => return None,
                Operation::Arithmetic {
                    sub,
                    flags,
                    immediate,
                } => {
                    let a = if immediate && rn == 31 {
                        regs.sp & mask
                    } else {
                        a
                    };
                    let b = if immediate {
                        u64::from(w >> 10 & 4095) << (if w & (1 << 22) != 0 { 12 } else { 0 })
                    } else {
                        shift()
                    };
                    let value = if sub {
                        a.wrapping_sub(b)
                    } else {
                        a.wrapping_add(b)
                    } & mask;
                    if flags {
                        let sign = 1u64 << (bits - 1);
                        let c = if sub { a >= b } else { value < a };
                        let v = (if sub { a ^ b } else { !(a ^ b) }) & (a ^ value) & sign != 0;
                        regs.nzcv = nz(value, sign) | (u32::from(c) << 29) | (u32::from(v) << 28);
                    }
                    if immediate && !flags && rd == 31 {
                        regs.sp = value;
                    } else {
                        set(regs, rd, value);
                    }
                }
                Operation::Logical => {
                    let b = if w & (1 << 21) != 0 {
                        !shift()
                    } else {
                        shift()
                    };
                    let op = w >> 29 & 3;
                    let value = match op {
                        1 => a | b,
                        2 => a ^ b,
                        _ => a & b,
                    } & mask;
                    set(regs, rd, value);
                    if op == 3 {
                        regs.nzcv = nz(value, 1u64 << (bits - 1));
                    }
                }
                Operation::Select => {
                    let value = if crate::fetched_instruction::condition(w >> 12 & 15, regs.nzcv) {
                        a
                    } else {
                        let b = if w & (1 << 30) != 0 { !b } else { b };
                        b.wrapping_add(u64::from(w >> 10 & 1))
                    };
                    set(regs, rd, value & mask);
                }
                Operation::Branch {
                    displacement,
                    condition,
                } => {
                    if crate::fetched_instruction::condition(condition, regs.nzcv) {
                        regs.pc = regs.pc.wrapping_add_signed(displacement);
                        return Some(Outcome::Branch);
                    }
                }
                Operation::RegisterBranch {
                    displacement,
                    mask,
                    nonzero,
                } => {
                    if (get(regs, rd) & mask != 0) == nonzero {
                        regs.pc = regs.pc.wrapping_add_signed(displacement);
                        return Some(Outcome::Branch);
                    }
                }
            }
        }
        None
    }
}

fn nz(value: u64, sign: u64) -> u32 {
    (u32::from(value & sign != 0) << 31) | (u32::from(value == 0) << 30)
}
pub fn get(regs: &Registers, n: usize) -> u64 {
    regs.x.get(n).copied().unwrap_or(0)
}
pub fn set(regs: &mut Registers, n: usize, value: u64) {
    if let Some(r) = regs.x.get_mut(n) {
        *r = value
    }
}

/// Complete a register-only window using a real hardware reservation.
///
/// A changed load discards the speculative register state. A conflicting
/// write after the hardware load produces the architectural STXR failure.
///
/// # Safety
/// The caller must hold a checked, writable, naturally aligned normal-RAM
/// alias valid for the entire load/store width and duration of this call.
pub unsafe fn commit(
    ptr: *mut u8,
    load: Access,
    store: Access,
    expected: u64,
    value: u64,
) -> Option<u32> {
    debug_assert!(load.load && !store.load && load.width == store.width);
    macro_rules! pair {
        ($ld:literal,$st:literal,$size:literal) => {{
            let status:u32;
            unsafe { core::arch::asm!(
                concat!($ld, " {sample", $size, "}, [{ptr}]"),
                concat!("cmp {sample", $size, "}, {expected", $size, "}"),
                "b.ne 2f",
                concat!($st, " {status:w}, {value", $size, "}, [{ptr}]"),
                "b 3f", "2:", "clrex", "mov {status:w}, #2", "3:",
                sample=out(reg) _, status=out(reg) status, ptr=in(reg) ptr,
                expected=in(reg) expected, value=in(reg) value, options(nostack)
            ); }
            (status != 2).then_some(status)
        }}
    }
    macro_rules! ordered {
        ($ld:literal,$lda:literal,$st:literal,$stl:literal,$size:literal) => {
            match (load.ordered, store.ordered) {
                (false, false) => pair!($ld, $st, $size),
                (true, false) => pair!($lda, $st, $size),
                (false, true) => pair!($ld, $stl, $size),
                (true, true) => pair!($lda, $stl, $size),
            }
        };
    }
    match load.width {
        1 => ordered!("ldxrb", "ldaxrb", "stxrb", "stlxrb", ":w"),
        2 => ordered!("ldxrh", "ldaxrh", "stxrh", "stlxrh", ":w"),
        4 => ordered!("ldxr", "ldaxr", "stxr", "stlxr", ":w"),
        8 => ordered!("ldxr", "ldaxr", "stxr", "stlxr", ":x"),
        _ => unreachable!(),
    }
}
