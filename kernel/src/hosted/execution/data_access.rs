//! Data aborts on host pages whose guest quarters cannot share native data
//! permissions. Check the whole transfer before committing register writeback
//! or touching memory. Ordinary host-representable accesses stay on the CPU.
use crate::{
    arch::Trapframe,
    environment::PAGE_SIZE,
    object::capability::memory_mapping::{AccessKind, AccessOp},
    task::Task,
    vm::{manager::MappingTransaction, vmem::VirtualMemoryPermission as Perm},
};
use core::sync::atomic::{Ordering, fence};

#[path = "exclusive_access.rs"]
mod exclusive_access;

struct Transfer {
    address: u64,
    width: usize,
    registers: [usize; 4],
    register_count: usize,
    writeback: Option<(usize, u64)>,
    load: bool,
    vector: bool,
    signed: bool,
    narrow: bool,
    ordered: bool,
}

fn signed(value: u64, bits: u32) -> u64 {
    (((value << (64 - bits)) as i64) >> (64 - bits)) as u64
}

fn decode(word: u32, tf: &Trapframe) -> Option<Transfer> {
    let rt = (word & 31) as usize;
    let rn = ((word >> 5) & 31) as usize;
    let base = tf.regs.reg.get(rn).map_or(tf.sp, |&n| n as u64);
    let size = word >> 30;
    let mut t = Transfer {
        address: base,
        width: 1 << size,
        registers: [rt; 4],
        register_count: 1,
        writeback: None,
        load: word & (1 << 22) != 0,
        vector: word & (1 << 26) != 0,
        signed: false,
        narrow: false,
        ordered: false,
    };
    if word & 0xbf20_0000 == 0x0c00_0000 {
        // AdvSIMD LD1/ST1 transfers a consecutive list of up to four vectors.
        t.register_count = match (word >> 12) & 15 {
            7 => 1,
            10 => 2,
            6 => 3,
            2 => 4,
            _ => return None,
        };
        t.width = if word & (1 << 30) == 0 { 8 } else { 16 };
        for (i, reg) in t.registers.iter_mut().enumerate() {
            *reg = (rt + i) % 32;
        }
        let rm = ((word >> 16) & 31) as usize;
        if word & (1 << 23) != 0 {
            let step = tf.regs.reg.get(rm).copied().unwrap_or(t.width * t.register_count);
            t.writeback = Some((rn, base.wrapping_add(step as u64)));
        } else if rm != 0 {
            return None;
        }
        return Some(t);
    }
    if word & 0x3f3f_fc00 == 0x081f_fc00 {
        // LDAR/STLR and their byte and halfword forms.
        if word & (1 << 23) == 0 {
            return None;
        }
        t.ordered = true;
        return Some(t);
    }
    if word & 0x3b00_0000 == 0x1800_0000 {
        // Load literal. The saved guest PC is the address used by the CPU.
        t.address = tf
            .elr
            .wrapping_add(signed(u64::from((word >> 5) & 0x7ffff), 19) << 2);
        t.load = true;
        t.width = match (size, t.vector) {
            (0, _) | (2, false) => 4,
            (1, _) => 8,
            (2, true) => 16,
            _ => return None,
        };
        t.signed = size == 2 && !t.vector;
        return Some(t);
    }
    if word & 0x3a00_0000 == 0x2800_0000 {
        // Pair, including non-temporal, offset, pre- and post-indexed.
        t.width = match (size, t.vector) {
            (0, _) => 4,
            (1, true) | (2, false) => 8,
            (2, true) => 16,
            (1, false) if t.load => {
                t.signed = true;
                4
            }
            _ => return None,
        };
        t.registers[1] = ((word >> 10) & 31) as usize;
        t.register_count = 2;
        let delta = signed(u64::from((word >> 15) & 127), 7).wrapping_mul(t.width as u64);
        let next = base.wrapping_add(delta);
        let mode = (word >> 23) & 3;
        if mode != 1 {
            t.address = next;
        }
        if mode & 1 != 0 {
            t.writeback = Some((rn, next));
        }
        return Some(t);
    }
    if word & 0x3a00_0000 != 0x3800_0000 {
        return None;
    }
    let opc = (word >> 22) & 3;
    if t.vector && opc >= 2 {
        if size != 0 {
            return None;
        }
        t.width = 16;
    } else if !t.vector && opc >= 2 {
        if size == 3 || (size == 2 && opc == 3) {
            return None;
        }
        t.load = true;
        t.signed = true;
        t.narrow = opc == 3;
    }
    if word & (1 << 24) != 0 {
        t.address = base.wrapping_add(u64::from((word >> 10) & 4095) * t.width as u64);
    } else if word & (1 << 21) != 0 {
        if (word >> 10) & 3 != 2 {
            return None;
        }
        let rm = ((word >> 16) & 31) as usize;
        let reg = tf.regs.reg.get(rm).copied().unwrap_or(0) as u64;
        let mut delta = match (word >> 13) & 7 {
            2 => reg as u32 as u64,
            3 | 7 => reg,
            6 => signed(reg & 0xffff_ffff, 32),
            _ => return None,
        };
        if word & (1 << 12) != 0 {
            delta = delta.wrapping_mul(t.width as u64);
        }
        t.address = base.wrapping_add(delta);
    } else {
        let mode = (word >> 10) & 3;
        let next = base.wrapping_add(signed(u64::from((word >> 12) & 511), 9));
        if mode != 1 {
            t.address = next;
        }
        if mode & 1 != 0 {
            t.writeback = Some((rn, next));
        }
    }
    Some(t)
}

fn reg_value(task: &Task, tf: &Trapframe, reg: usize, vector: bool) -> u128 {
    if vector {
        let [lo, hi] = task.vcpu.lock().fpu.v[reg];
        u128::from(lo) | (u128::from(hi) << 64)
    } else {
        tf.regs.reg.get(reg).copied().unwrap_or(0) as u128
    }
}

fn set_value(task: &Task, tf: &mut Trapframe, t: &Transfer, reg: usize, value: u128) {
    if t.vector {
        task.vcpu.lock().fpu.v[reg] = [value as u64, (value >> 64) as u64];
    } else if let Some(dst) = tf.regs.reg.get_mut(reg) {
        let raw = value as u64;
        let value = if t.signed {
            signed(raw, (t.width * 8) as u32)
        } else {
            raw
        };
        *dst = if t.narrow {
            value as u32 as usize
        } else {
            value as usize
        };
    }
}

pub(super) enum Result {
    Done,
    Fault(usize),
    Unsupported(u32),
}

fn alias(
    task: &Task,
    tx: &MappingTransaction<'_>,
    va: usize,
    size: usize,
    op: AccessOp,
) -> Option<usize> {
    let vm = &task.vm_manager;
    let map = vm.search_memory_map(va)?;
    if va.checked_add(size.checked_sub(1)?)? > map.vmarea.end {
        return None;
    }
    let permission = match op {
        AccessOp::Load => Perm::Read,
        AccessOp::Store => Perm::Write,
        AccessOp::Instruction => Perm::Execute,
    };
    if !Perm::User.contained_in(map.permissions)
        || !permission.contained_in(map.permissions & map.maximum_permissions)
    {
        return None;
    }
    let pa = if let Some(owner) = &map.owner {
        let access = AccessKind { op, vaddr: va, size: Some(size) };
        // An instruction already ran on this permit. For stable private text,
        // revalidate its current backing and execute grant under the transaction
        // without parking peers to reinstall an identical resident PTE.
        if op == AccessOp::Instruction
            && !map.is_shared
            && map.memory_attribute == crate::vm::vmem::MemoryAttribute::Normal
            && !owner.private_fault_requires_copy(&access)
        {
            let page = (va - map.vm_start) / PAGE_SIZE;
            let offset = va % PAGE_SIZE;
            if let Some(base) = owner.stable_physical_extent(page, 1)
                && let Some(pa) = base.checked_add(offset as u64)
                && let Some(pte) = vm.get_root_page_table().and_then(|mut table| table.resident_page(va))
                && pte.paddr == base
                && pte.memory_attribute == map.memory_attribute
                && Perm::User.contained_in(pte.permissions)
                && Perm::Execute.contained_in(pte.permissions)
                && let Ok(resolved) = owner.resolve_fault(
                    &AccessKind { op: AccessOp::Load, ..access },
                    page,
                    map.vm_start,
                )
                && !resolved.is_tail
                && resolved.paddr_page_base == base
                && Perm::Execute.contained_in(owner.fault_page_permissions(&access, map.permissions))
            {
                return crate::hosted::alias_for(pa);
            }
        }
        tx.fault(access).ok()?;
        vm.get_root_page_table()?.translate(va)?
    } else {
        if map.pmarea.start == 0 {
            return None;
        }
        map.pmarea
            .start
            .checked_add((va - map.vmarea.start) as u64)?
    };
    crate::hosted::alias_for(pa)
}

pub(super) fn data_address(address: u64) -> usize {
    if address & (1 << 55) == 0 {
        (address & 0x00ff_ffff_ffff_ffff) as usize
    } else {
        address as usize
    }
}

// The guest reads the host DCZID_EL0 when selecting its memset sequence.
// Perform its cache-block zero on the same coherent physical RAM.
fn zero_block(word: u32, task: &Task, tf: &mut Trapframe, tx: &MappingTransaction<'_>) -> Result {
    let dczid: usize;
    unsafe { core::arch::asm!("mrs {}, dczid_el0", out(reg) dczid, options(nomem, nostack, preserves_flags)) };
    let size = 4 << (dczid & 15);
    if dczid & 16 != 0 || size > PAGE_SIZE {
        return Result::Unsupported(word);
    }
    let reg = (word & 31) as usize;
    let address = data_address(tf.regs.reg.get(reg).copied().unwrap_or(0) as u64);
    let base = address & !(size - 1);
    if data_address(tf.far_el1) & !(size - 1) != base {
        return Result::Unsupported(word);
    }
    if !task.vm_manager.search_memory_map(base).is_some_and(|map|
        map.memory_attribute == crate::vm::vmem::MemoryAttribute::Normal)
    {
        return Result::Fault(address);
    }
    let Some(ptr) = alias(task, tx, base, size, AccessOp::Store) else {
        return Result::Fault(address);
    };
    if !ptr.is_multiple_of(size) {
        return Result::Unsupported(word);
    }
    unsafe { core::arch::asm!("dc zva, {}", in(reg) ptr, options(nostack, preserves_flags)) };
    tf.elr += 4;
    Result::Done
}

#[path = "pair_compare_exchange.rs"]
mod pair_compare_exchange;

// LSE accesses must remain atomic with a different worker's direct mapping
// of the same physical word. An address-space lock alone cannot provide that.
fn atomic(
    word: u32,
    task: &Task,
    tf: &mut Trapframe,
    tx: &MappingTransaction<'_>,
) -> Option<Result> {
    use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering::SeqCst};
    let pair = scarlet_hosted_runtime::exclusive::is_pair_compare_exchange(word);
    let cas = word & 0x3fa0_7c00 == 0x08a0_7c00;
    let op = (word >> 12) & 15;
    if !pair && !cas && (word & 0x3f20_0c00 != 0x3820_0000 || op > 8) {
        return None;
    }
    let rs = ((word >> 16) & 31) as usize;
    let rt = (word & 31) as usize;
    let rn = ((word >> 5) & 31) as usize;
    let va = data_address(tf.regs.reg.get(rn).map_or(tf.sp, |&r| r as u64));
    let width = if pair { 8 << (word >> 30) } else { 1 << (word >> 30) };
    if va != data_address(tf.far_el1) || va % width != 0 {
        return Some(Result::Unsupported(word));
    }
    // Both access types are required even when the comparison fails.
    if alias(task, tx, va, width, AccessOp::Load).is_none() {
        return Some(Result::Fault(va));
    }
    let Some(ptr) = alias(task, tx, va, width, AccessOp::Store) else {
        return Some(Result::Fault(va));
    };
    if pair {
        pair_compare_exchange::apply(tf, rs, rt, ptr, width);
        tf.elr += 4;
        return Some(Result::Done);
    }
    let operand = tf.regs.reg.get(rs).copied().unwrap_or(0);
    let replacement = tf.regs.reg.get(rt).copied().unwrap_or(0);
    macro_rules! apply {
        ($atomic:ty, $uint:ty, $int:ty) => {{
            let dst = unsafe { <$atomic>::from_ptr(ptr as *mut $uint) };
            let value = operand as $uint;
            (if cas {
                dst.compare_exchange(value, replacement as $uint, SeqCst, SeqCst)
                    .unwrap_or_else(|old| old)
            } else {
                match op {
                    0 => dst.fetch_add(value, SeqCst),
                    1 => dst.fetch_and(!value, SeqCst),
                    2 => dst.fetch_xor(value, SeqCst),
                    3 => dst.fetch_or(value, SeqCst),
                    4 | 5 => dst
                        .fetch_update(SeqCst, SeqCst, |n| {
                            Some(if op == 4 {
                                (n as $int).max(value as $int) as $uint
                            } else {
                                (n as $int).min(value as $int) as $uint
                            })
                        })
                        .unwrap(),
                    6 => dst.fetch_max(value, SeqCst),
                    7 => dst.fetch_min(value, SeqCst),
                    _ => dst.swap(value, SeqCst),
                }
            }) as usize
        }};
    }
    let old = match width {
        1 => apply!(AtomicU8, u8, i8),
        2 => apply!(AtomicU16, u16, i16),
        4 => apply!(AtomicU32, u32, i32),
        _ => apply!(AtomicU64, u64, i64),
    };
    if let Some(reg) = tf.regs.reg.get_mut(if cas { rs } else { rt }) {
        *reg = old;
    }
    tf.elr += 4;
    Some(Result::Done)
}

pub(super) fn complete(
    task: &Task,
    tf: &mut Trapframe,
    tx: &MappingTransaction<'_>,
    window: Option<&scarlet_hosted_runtime::exclusive::Window>,
    fault_instruction: Option<u32>,
) -> Result {
    let pc = tf.elr as usize;
    if pc % 4 != 0 {
        return Result::Fault(pc);
    }
    let Some(instruction) = alias(task, tx, pc, 4, AccessOp::Instruction) else {
        return Result::Fault(pc);
    };
    // The worker retains the word that actually ran, including code fetched
    // from an immutable generation through a writable alias in another task.
    let word = fault_instruction.unwrap_or_else(|| unsafe { (instruction as *const u32).read() });
    if let Some(access) = scarlet_hosted_runtime::exclusive::Access::decode(word) {
        return exclusive_access::complete(word, access, window, task, tf, tx);
    }
    if word & !31 == 0xd50b_7420 {
        return zero_block(word, task, tf, tx);
    }
    if let Some(result) = atomic(word, task, tf, tx) {
        return result;
    }
    let Some(t) = decode(word, tf) else {
        return Result::Unsupported(word);
    };
    let registers = &t.registers[..t.register_count];
    let n = t.width * registers.len();
    let address = data_address(t.address);
    let Some(lim) = address.checked_add(n) else {
        return Result::Fault(address);
    };
    if !(address..lim).contains(&data_address(tf.far_el1)) || t.load == (tf.esr_el1 & (1 << 6) != 0)
    {
        return Result::Unsupported(word);
    }
    if t.ordered && address % t.width != 0 {
        return Result::Unsupported(word);
    }
    // At most two guest pages. A denied second quarter must not change any
    // byte in the first quarter or advance a pre/post-indexed base register.
    let mut chunks = [(0, 0, 0); 2];
    let mut done = 0;
    for chunk in &mut chunks {
        if done == n {
            break;
        }
        let va = address + done;
        let count = (PAGE_SIZE - va % PAGE_SIZE).min(n - done);
        let op = if t.load {
            AccessOp::Load
        } else {
            AccessOp::Store
        };
        let alias = alias(task, tx, va, count, op);
        let Some(alias) = alias else {
            return Result::Fault(va);
        };
        *chunk = (alias, done, count);
        done += count;
    }
    let mut bytes = [0u8; 64];
    if !t.load {
        for (i, &reg) in registers.iter().enumerate() {
            let value = reg_value(task, tf, reg, t.vector).to_le_bytes();
            bytes[i * t.width..(i + 1) * t.width].copy_from_slice(&value[..t.width]);
        }
        if t.ordered {
            fence(Ordering::SeqCst);
        }
    }
    for (alias, offset, len) in chunks {
        if len == 0 {
            break;
        }
        // Preserve single-copy atomicity of aligned scalar transfers to a
        // shared alias. Split unaligned accesses only at real backing edges.
        let mut copied = 0;
        while copied < len {
            let count = (len - copied).min(t.width);
            let slot = &mut bytes[offset + copied..offset + copied + count];
            let ptr = (alias + copied) as *mut u8;
            unsafe {
                transfer(ptr, slot, t.load);
            }
            copied += count;
        }
    }
    if t.load {
        if t.ordered {
            fence(Ordering::SeqCst);
        }
        for (i, &reg) in registers.iter().enumerate() {
            let mut value = [0; 16];
            value[..t.width].copy_from_slice(&bytes[i * t.width..(i + 1) * t.width]);
            set_value(task, tf, &t, reg, u128::from_le_bytes(value));
        }
    }
    if let Some((rn, value)) = t.writeback {
        if rn == 31 {
            tf.sp = value;
        } else {
            tf.regs.reg[rn] = value as usize;
        }
    }
    tf.elr += 4;
    Result::Done
}

// All pointers are VM-owned, validated against the operation and held by the
// transaction. Use native accesses for aligned words shared with other workers.
unsafe fn transfer(ptr: *mut u8, bytes: &mut [u8], load: bool) {
    macro_rules! word {
        ($ty:ty) => {{
            if load {
                bytes.copy_from_slice(&unsafe { ptr.cast::<$ty>().read_volatile() }.to_le_bytes());
            } else {
                unsafe {
                    ptr.cast::<$ty>()
                        .write_volatile(<$ty>::from_le_bytes(bytes.try_into().unwrap()))
                };
            }
        }};
    }
    match bytes.len() {
        1 => word!(u8),
        2 if ptr.addr() % 2 == 0 => word!(u16),
        4 if ptr.addr() % 4 == 0 => word!(u32),
        8 if ptr.addr() % 8 == 0 => word!(u64),
        16 if ptr.addr() % 16 == 0 => word!(u128),
        n if load => unsafe { core::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), n) },
        n => unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, n) },
    }
}
