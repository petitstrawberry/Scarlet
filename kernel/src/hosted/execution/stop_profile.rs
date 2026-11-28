//! Optional per-vCPU sampling of actual worker replies, before fault handling.
use crate::{arch::Trapframe, task::Task};
use std::{cell::RefCell, collections::BTreeMap, sync::LazyLock, time::Instant};

struct Counts {
    since: Instant,
    total: u64,
    data_aborts: u64,
    next: u64,
    samples: BTreeMap<(usize, u64, u64), u64>,
}

pub(super) fn record(task: &Task, frame: &Trapframe, interrupted: bool) {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("SCARLET_PROFILE_WORKER_STOPS").is_some());
    if !*ENABLED {
        return;
    }
    std::thread_local! {
        static PROFILE: RefCell<Counts> = RefCell::new(Counts {
            since: Instant::now(), total: 0, data_aborts: 0, next: 1, samples: BTreeMap::new(),
        });
    }
    PROFILE.with_borrow_mut(|c| {
        c.total += 1;
        c.data_aborts += u64::from(!interrupted && frame.esr_el1 >> 26 == 0x24);
        if c.total != c.next {
            return;
        }
        let kind = if interrupted { u64::MAX } else { frame.esr_el1 >> 26 };
        let site = match kind {
            u64::MAX => 0,
            0x15 => frame.get_syscall_number() as u64,
            _ => frame.elr,
        };
        *c.samples.entry((task.get_id(), kind, site)).or_default() += 1;
        #[cfg(not(target_os = "ios"))]
        if kind == 0x24 && c.total % 16384 < 64 {
            let far = super::data_access::data_address(frame.far_el1);
            if let Some(m) = task.vm_manager.search_memory_map(far) {
                let page = (far - m.vm_start) / crate::environment::PAGE_SIZE;
                let backing = m.owner.as_ref().and_then(|o| o.stable_physical_extent(page, 1));
                std::eprintln!("[worker-data] tid={} tgid={} pc={:#x} far={:#x} write={} map={:#x}..={:#x} origin={:#x} pa={:#x} owner={:?} resident={:x?} perms={:#x} shared={}",
                    task.get_id(), task.get_thread_group_id(), site, far, frame.esr_el1 & 64 != 0,
                    m.vmarea.start, m.vmarea.end, m.vm_start, m.pmarea.start,
                    m.owner.as_ref().map(|o| o.mmap_owner_name()), backing, m.permissions, m.is_shared);
            }
        }
        // Vary the stride to avoid aliasing short periodic syscall sequences.
        c.next += 53 + (c.total % 23);
        let elapsed = c.since.elapsed();
        if elapsed.as_secs() >= 2 {
            let mut counts: alloc::vec::Vec<_> = core::mem::take(&mut c.samples).into_iter().collect();
            counts.sort_unstable_by_key(|(_, n)| core::cmp::Reverse(*n));
            counts.truncate(12);
            std::eprintln!("[worker-stops] cpu={} seconds={:.3} total={} sampled(task,ec,site):count={:x?}",
                crate::arch::get_cpu().get_cpuid(), elapsed.as_secs_f64(), c.total, counts);
            std::eprintln!("[worker-data-count] cpu={} data={} all={}",
                crate::arch::get_cpu().get_cpuid(), c.data_aborts, c.total);
            c.since = Instant::now();
            c.total = 0;
            c.data_aborts = 0;
            c.next = 1;
        }
    });
}
