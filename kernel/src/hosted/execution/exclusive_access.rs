//! A mediated scalar exclusive window linearizes on the real checked RAM
//! alias. Register-only work is private until the operation completes. An
//! interrupted or detached store-exclusive never inherits a software monitor.
use super::{AccessOp, MappingTransaction, Result, Task, Trapframe, alias, data_address};
use crate::vm::vmem::MemoryAttribute;
use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use scarlet_hosted_runtime::{
    exclusive::{self, Access, Outcome, Window},
    worker_context::Registers,
};

fn restore(tf: &mut Trapframe, r: &Registers) {
    tf.regs.reg = r.x.map(|v| v as usize);
    tf.sp = r.sp;
    tf.elr = r.pc;
    tf.spsr = (tf.spsr & !0xf000_0000) | u64::from(r.nzcv & 0xf000_0000);
}

fn observed(ptr: usize, load: Access) -> u64 {
    let order = if load.ordered {
        Ordering::Acquire
    } else {
        Ordering::Relaxed
    };
    unsafe {
        match load.width {
            1 => AtomicU8::from_ptr(ptr as *mut u8).load(order).into(),
            2 => AtomicU16::from_ptr(ptr as *mut u16).load(order).into(),
            4 => AtomicU32::from_ptr(ptr as *mut u32).load(order).into(),
            8 => AtomicU64::from_ptr(ptr as *mut u64).load(order),
            _ => unreachable!(),
        }
    }
}

pub(super) fn complete(
    word: u32,
    fault: Access,
    window: Option<&Window>,
    task: &Task,
    tf: &mut Trapframe,
    tx: &MappingTransaction<'_>,
) -> Result {
    let far = data_address(tf.far_el1);
    let write = tf.esr_el1 & (1 << 6) != 0;
    let base = data_address(tf.regs.reg.get(fault.base).map_or(tf.sp, |&v| v as u64));
    if base != far
        || write == fault.load
        || !far.is_multiple_of(fault.width)
        || !task.vm_manager.search_memory_map(far).is_some_and(|m| {
            m.memory_attribute == MemoryAttribute::Normal && far + (fault.width - 1) <= m.vmarea.end
        })
    {
        return Result::Fault(far);
    }
    let Some(window) = window else {
        if fault.load {
            let Some(ptr) = alias(task, tx, far, fault.width, AccessOp::Load) else {
                return Result::Fault(far);
            };
            let value = observed(ptr, fault);
            if let Some(destination) = tf.regs.reg.get_mut(fault.value) {
                *destination = value as usize;
            }
            tf.elr += 4;
            return Result::Done;
        }
        // A guest stop, signal or direct entry lost the local monitor.
        // A real retry is an independent load and may observe any new value.
        if alias(task, tx, far, fault.width, AccessOp::Store).is_none() {
            return Result::Fault(far);
        }
        if let Some(status) = tf.regs.reg.get_mut(fault.status) {
            *status = 1;
        }
        tf.elr += 4;
        return Result::Done;
    };
    let initial = &window.initial;
    let Some(load) = window.words.first().copied().and_then(Access::decode) else {
        return Result::Unsupported(word);
    };
    let index = if fault.load {
        0
    } else {
        window.words.len() - 1
    };
    if initial.pc.checked_add((4 * index) as u64) != Some(tf.elr)
        || window.words[index] != word
        || data_address(load.address(initial)) != far
    {
        return Result::Unsupported(word);
    }
    // The worker retained the instruction words from the stopped generation.
    // Revalidate executable authority under the current VM transaction.
    for i in 0..window.words.len() {
        let va = initial.pc as usize + i * 4;
        if alias(task, tx, va, 4, AccessOp::Instruction).is_none() {
            return Result::Unsupported(word);
        }
    }
    let Some(ptr) = alias(task, tx, far, load.width, AccessOp::Load) else {
        restore(tf, initial);
        tf.esr_el1 &= !(1 << 6);
        return Result::Fault(far);
    };
    let old = observed(ptr, load);
    let mut regs = *initial;
    match window.evaluate(old, &mut regs) {
        Some(Outcome::Branch) => {
            restore(tf, &regs);
            Result::Done
        }
        Some(Outcome::Store(store)) => {
            let store_va = data_address(store.address(&regs));
            if store_va != far {
                return Result::Unsupported(word);
            }
            let Some(ptr) = alias(task, tx, far, store.width, AccessOp::Store) else {
                restore(tf, &regs);
                tf.esr_el1 = 0x9200_004f;
                return Result::Fault(far);
            };
            // SAFETY: alias checked write permissions, alignment and the full RAM range above.
            if let Some(status) = unsafe {
                exclusive::commit(
                    ptr as *mut u8,
                    load,
                    store,
                    old,
                    exclusive::get(&regs, store.value),
                )
            } {
                exclusive::set(&mut regs, store.status, status.into());
                regs.pc += 4;
                restore(tf, &regs);
            } else {
                restore(tf, initial);
            }
            Result::Done
        }
        None => Result::Unsupported(word),
    }
}
