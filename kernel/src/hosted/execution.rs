//! Native guest execution and trap dispatch for a scheduled Task.
mod admission_cache;
#[cfg(not(target_os = "ios"))]
mod boundary;
#[cfg(not(target_os = "ios"))]
mod data_access;
mod stop_profile;
#[cfg(target_os = "ios")]
pub(super) mod in_process;
#[cfg(target_os = "ios")]
pub(super) use in_process::InterruptTarget;

use crate::vm::vmem::{VirtualMemoryMap, VirtualMemoryPermission as Perm};
use crate::{
    arch::Trapframe,
    sched::scheduler,
    task::Task,
    vm::vmem::MappingOrigin,
};
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{
    fmt,
    ops::Range,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};
use scarlet_hosted_runtime::worker_context::{GuestStop, Registers};
#[cfg(not(target_os = "ios"))]
use scarlet_hosted_runtime::{
    run_gate::{MappingOutcome, RunGate, RunPermit, TaskId, WorkerId},
    worker::{Worker, WorkerControl, WorkerProcess, WorkerThread, identity},
    worker_memory::Protection,
};
use std::sync::Mutex;
#[cfg(not(target_os = "ios"))]
use std::{io, os::fd::AsFd};

#[derive(Default)]
pub(crate) struct Image {
    publication_generation: AtomicU64,
    guest_page_size: AtomicUsize,
    code: Mutex<Vec<(MappingOrigin, Range<usize>)>>,
    file_code: Mutex<BTreeMap<MappingOrigin, Arc<[Range<usize>]>>>,
    #[cfg(not(target_os = "ios"))]
    session: Mutex<Option<Session>>,
    #[cfg(target_os = "ios")]
    local: Mutex<Option<Arc<in_process::LocalImage>>>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostedImage")
    }
}

#[cfg(not(target_os = "ios"))]
#[derive(Clone, PartialEq, Eq)]
struct Extent {
    origin: MappingOrigin,
    vm_start: usize,
    pa: u64,
    len: usize,
    perms: usize,
    republish: bool,
    parts: Vec<boundary::Part>,
}

impl admission_cache::Snapshot for crate::vm::vmem::VirtualMemoryMap {
    fn key(&self) -> usize {
        self.vmarea.start
    }

    fn needs_owner_check(&self) -> bool {
        self.owner
            .as_ref()
            .is_some_and(|owner| !owner.has_fixed_publication())
    }

    fn same(&self, other: &Self) -> bool {
        self.vmarea.start == other.vmarea.start
            && self.vmarea.end == other.vmarea.end
            && self.pmarea.start == other.pmarea.start
            && self.pmarea.end == other.pmarea.end
            && self.vm_start == other.vm_start
            && self.mapping_origin == other.mapping_origin
            && self.permissions == other.permissions
            && self.maximum_permissions == other.maximum_permissions
            && self.is_shared == other.is_shared
            && self.memory_attribute == other.memory_attribute
            && match (&self.owner, &other.owner) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

#[cfg(not(target_os = "ios"))]
struct Session {
    interrupts: Arc<InterruptTarget>,
    worker: Worker,
    gate: Arc<RunGate>,
    id: WorkerId,
    // VMAs and each owner's resident spans arrive in ascending virtual address.
    installed: Vec<(usize, Extent)>,
    desired: Vec<(usize, Extent)>,
    admission: admission_cache::Cache<crate::vm::vmem::VirtualMemoryMap, Vec<(usize, Extent)>>,
}

/// Follows the Scarlet Task across vCPU migration. Exec selects a new endpoint
/// by image identity; dropping the Task closes its worker thread.
#[cfg(not(target_os = "ios"))]
#[derive(Default, Debug)]
pub(crate) struct TaskThread(Mutex<Option<(WorkerId, WorkerThread)>>);

#[cfg(not(target_os = "ios"))]
struct Running {
    permit: RunPermit,
    interrupts: Arc<InterruptTarget>,
}

/// Remains valid for exactly one worker lifetime, even across an exec.
#[cfg(not(target_os = "ios"))]
pub(super) struct InterruptTarget {
    control: Arc<WorkerControl>,
    gate: Arc<RunGate>,
    process: Arc<WorkerProcess>,
}

#[cfg(not(target_os = "ios"))]
impl InterruptTarget {
    pub(super) fn request(&self, permit: RunPermit) {
        if self.control.interrupt(permit).is_err() {
            self.process
                .terminate()
                .expect("Cannot confirm worker death");
            self.gate.confirm_worker_death();
        }
    }
}

#[cfg(not(target_os = "ios"))]
impl Running {
    fn receive(
        self,
        thread: &mut WorkerThread,
    ) -> io::Result<scarlet_hosted_runtime::worker::Stop> {
        match thread.receive_stop() {
            Ok(stop)
                if stop.identity == identity(self.permit)
                    && self.interrupts.gate.acknowledge_stop(self.permit) =>
            {
                Ok(stop)
            }
            reply => {
                self.interrupts.process.terminate()?;
                self.interrupts.gate.confirm_worker_death();
                Err(reply
                    .err()
                    .unwrap_or_else(|| io::Error::other("Invalid worker stop identity")))
            }
        }
    }
}

impl Image {
    pub(crate) fn invalidate_publication(&self) {
        self.publication_generation
            .fetch_update(Ordering::Release, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("Hosted publication generation exhausted");
    }

    pub(crate) fn set_guest_page_size(&self, size: usize) {
        self.guest_page_size.store(size, Ordering::Relaxed);
        self.invalidate_publication();
    }

    pub(crate) fn guest_page_size(&self) -> usize {
        match self.guest_page_size.load(Ordering::Relaxed) {
            0 => crate::vm::execution_page_size(),
            size => size,
        }
    }

    pub(crate) fn add_code(&self, origin: MappingOrigin, range: Range<usize>) {
        self.code.lock().unwrap().push((origin, range));
        self.invalidate_publication();
    }

    #[cfg(not(target_os = "ios"))]
    fn append_instruction_ranges(
        &self,
        origin: MappingOrigin,
        vm_start: usize,
        fragment: Range<usize>,
        base: usize,
        output: &mut Vec<Range<usize>>,
    ) {
        let mut append = |start: usize, end: usize| {
            let lo = start.max(fragment.start);
            let hi = end.min(fragment.end);
            if lo < hi {
                output.push(lo - base..hi - base);
            }
        };
        for (_, range) in self
            .code
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| *id == origin)
        {
            append(range.start, range.end);
        }
        if let Some(file) = self.file_code.lock().unwrap().get(&origin) {
            for range in file.iter() {
                append(vm_start + range.start, vm_start + range.end);
            }
        }
    }

    pub(crate) fn fork_into(&self, child: &crate::vm::manager::VirtualMemoryManager) -> Result<(), &'static str> {
        let image = child.hosted_image();
        image.set_guest_page_size(self.guest_page_size());
        *image.code.lock().unwrap() = self.code.lock().unwrap().clone();
        *image.file_code.lock().unwrap() = self.file_code.lock().unwrap().clone();
        #[cfg(target_os = "ios")]
        self.fork_signed_into(child)?;
        Ok(())
    }

    pub(crate) fn retain_file_code(&self, map: &crate::vm::vmem::VirtualMemoryMap) {
        if let Some(ranges) = map.owner.as_ref().and_then(|o| o.instruction_ranges()) {
            self.file_code
                .lock()
                .unwrap()
                .entry(map.mapping_origin)
                .or_insert(ranges);
        }
    }

    /// A writer can change code and restore RX before any thread reenters.
    /// Force a fresh snapshot even when address, frame and final flags match.
    pub(crate) fn invalidate_code(&self, start: usize, end_inclusive: usize) {
        self.invalidate_publication();
        #[cfg(target_os = "ios")]
        let _ = (start, end_inclusive);
        #[cfg(not(target_os = "ios"))]
        if let Some(session) = self.session.lock().unwrap().as_mut() {
            let first = session
                .installed
                .partition_point(|(base, extent)| base.saturating_add(extent.len - 1) < start);
            for (base, extent) in &mut session.installed[first..] {
                if *base > end_inclusive {
                    break;
                }
                if Perm::Execute.contained_in(extent.perms) {
                    extent.republish = true;
                }
            }
        }
    }

    /// The VM transaction excludes new admissions even for a nonblocking
    /// legacy translation. Never wait for a worker under an IRQ guard.
    #[cfg(not(target_os = "ios"))]
    pub(crate) fn try_parked(&self) -> bool {
        self.session.try_lock().is_ok_and(|session| {
            session
                .as_ref()
                .is_none_or(|session| session.gate.try_parked())
        })
    }

    /// The caller owns the VM transaction, so new guest entries cannot race
    /// this snapshot. An acknowledgement never needs that transaction.
    #[cfg(not(target_os = "ios"))]
    pub(crate) fn quiesce(&self) {
        let mut session = self.session.lock().unwrap();
        if let Some(session) = session.as_mut() {
            let gate = session.gate.clone();
            // The caller owns the mapping transaction. Once all permits have
            // stopped, another thread cannot enter while it changes the VM.
            if gate.try_parked() {
                return;
            }
            if let Ok(barrier) = gate.close_for_mapping() {
                for &permit in barrier.issued_permits() {
                    if session.worker.interrupt(permit).is_err() {
                        session.retire();
                        return;
                    }
                }
                if let Ok(parked) = barrier.wait() {
                    // New entries still need the VM transaction and must
                    // install changed mappings before issuing another permit.
                    let _ = parked.finish_after_mapping_ack(MappingOutcome::Restored);
                }
            }
        }
    }

    #[cfg(not(target_os = "ios"))]
    fn run(self: &Arc<Self>, task: &Task, tf: &mut Trapframe) -> io::Result<GuestRun> {
        let mut endpoint = task.hosted_thread.0.lock().unwrap().take();
        let running = {
            let vm = &task.vm_manager;
            let _transaction = vm.mapping_transaction();
            let mut session = self.session.lock().unwrap();
            if session.is_none() {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                let id = WorkerId {
                    address_space: vm.get_asid() as usize,
                    image_generation: NEXT.fetch_add(1, Ordering::Relaxed),
                };
                let worker = Worker::start()?;
                let gate = Arc::new(RunGate::new(id));
                *session = Some(Session {
                    interrupts: Arc::new(InterruptTarget {
                        control: worker.control(),
                        process: worker.process(),
                        gate: gate.clone(),
                    }),
                    id,
                    worker,
                    gate,
                    installed: Vec::new(),
                    desired: Vec::new(),
                    admission: Default::default(),
                });
            }
            let session = session.as_mut().unwrap();
            session
                .admit(self, task, tf, &mut endpoint)
                .inspect_err(|_| session.retire())?
        };
        let permit = running.permit;
        super::interrupts::guest_entered(permit, Arc::downgrade(&running.interrupts));
        let stop = running.receive(&mut endpoint.as_mut().unwrap().1);
        super::interrupts::guest_stopped(permit);
        let stop = stop?;
        *task.hosted_thread.0.lock().unwrap() = endpoint;
        let guest = stop.guest;
        store_stop(task, tf, &guest);
        if !guest.interrupted && tf.esr_el1 >> 26 == 0x24 {
            let address = data_access::data_address(tf.far_el1);
            let tx = task.vm_manager.mapping_transaction();
            let boundary = self
                .session
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|session| {
                    let end = session.installed.partition_point(|(va, _)| *va <= address);
                    session.installed[..end]
                        .last()
                        .is_some_and(|(base, extent)| {
                            address - *base < extent.len && !extent.parts.is_empty()
                        })
                });
            if boundary {
                let required = if tf.esr_el1 & (1 << 6) != 0 {
                    Perm::Write
                } else {
                    Perm::Read
                };
                if task.vm_manager.search_memory_map(address).is_some_and(|m| {
                    Perm::User.contained_in(m.permissions) && required.contained_in(m.permissions)
                }) {
                    match data_access::complete(
                        task,
                        tf,
                        &tx,
                        stop.exclusive.as_ref(),
                        stop.fault_instruction,
                    ) {
                        data_access::Result::Done => return Ok(GuestRun::Resume),
                        data_access::Result::Fault(denied) => tf.far_el1 = denied as u64,
                        data_access::Result::Unsupported(word) => {
                            return Err(io::Error::other(alloc::format!(
                                "Unsupported AArch64 boundary access {word:#010x} at {:#x} to {address:#x}",
                                tf.elr
                            )));
                        }
                    }
                }
            }
        }
        Ok(GuestRun::from_stop(&guest))
    }
}

/// Scan two address-ordered snapshots, advancing each cursor at most once.
#[cfg(not(target_os = "ios"))]
fn changed_extents<'a>(
    snapshot: &'a [(usize, Extent)],
    mut previous: &'a [(usize, Extent)],
) -> impl Iterator<Item = &'a (usize, Extent)> {
    snapshot.iter().filter(move |entry| {
        while let Some((first, rest)) = previous.split_first() {
            if first.0 >= entry.0 {
                break;
            }
            previous = rest;
        }
        previous.first() != Some(*entry)
    })
}

#[cfg(not(target_os = "ios"))]
impl Session {
    fn retire(&mut self) {
        self.worker
            .terminate()
            .expect("Cannot confirm worker death");
        self.gate.confirm_worker_death();
    }

    fn admit(
        &mut self,
        image: &Image,
        task: &Task,
        tf: &Trapframe,
        endpoint: &mut Option<(WorkerId, WorkerThread)>,
    ) -> io::Result<Running> {
        let vm = &task.vm_manager;
        let cpu = crate::arch::get_cpu().get_cpuid();
        let page = crate::vm::execution_page_size();
        let desired = &mut self.desired;
        let changed = self.admission.update(
            image.publication_generation.load(Ordering::Acquire),
            || {
                vm.memmaps_iter_with(|maps| {
                    maps.filter(|m| {
                        Perm::User.contained_in(m.permissions) && m.permissions & 7 != 0
                    })
                    .cloned()
                    .collect()
                })
            },
            |m| match &m.owner {
                Some(owner) => owner.publication_generation(),
                None => Some(0),
            },
            |m| {
                let mut extents = Vec::new();
                image.retain_file_code(m);
                visit_resident_extents(m, |address, pa, len, perms| {
                    extents.push((
                        address,
                        Extent {
                            origin: m.mapping_origin,
                            vm_start: m.vm_start,
                            pa,
                            len,
                            perms,
                            parts: Vec::new(),
                            republish: false,
                        },
                    ));
                });
                extents
            },
        );
        if changed {
            desired.clear();
            for extents in self.admission.values() {
                for (address, extent) in extents {
                    boundary::insert(desired, page, *address, extent.clone());
                }
            }
        }
        if changed && *desired != self.installed {
            let barrier = self
                .gate
                .close_for_mapping()
                .map_err(|_| io::Error::other("Worker unavailable"))?;
            for &permit in barrier.issued_permits() {
                self.worker.interrupt(permit)?;
            }
            let parked = barrier
                .wait()
                .map_err(|_| io::Error::other("Worker terminated"))?;
            for &(address, _) in changed_extents(&self.installed, desired) {
                self.worker.unmap(address)?;
            }
            for &(address, ref extent) in changed_extents(desired, &self.installed) {
                if !extent.parts.is_empty() {
                    boundary::publish(&mut self.worker, image, address, extent)?;
                    continue;
                }
                let (file, offset) = super::memory::worker_backing(extent.pa, extent.len)?;
                let mut ranges = Vec::new();
                let protection = if Perm::Execute.contained_in(extent.perms) {
                    if Perm::Write.contained_in(extent.perms) {
                        return Err(io::Error::other("Writable executable mapping"));
                    }
                    image.append_instruction_ranges(
                        extent.origin,
                        extent.vm_start,
                        address..address + extent.len,
                        address,
                        &mut ranges,
                    );
                    if ranges.is_empty() {
                        // ELF section tables are optional at run time. Preserve original
                        // data bytes and translate only words the guest actually fetches.
                        ranges.push(0..extent.len);
                        Protection::ReadExecuteOnFetch(&ranges)
                    } else {
                        ranges.sort_by_key(|range| range.start);
                        Protection::ReadExecute(&ranges)
                    }
                } else if Perm::Write.contained_in(extent.perms) {
                    Protection::ReadWrite
                } else {
                    Protection::ReadOnly
                };
                self.worker
                    .map(address, extent.len, file.as_fd(), offset, protection)
                    .map_err(|e| {
                        io::Error::other(alloc::format!(
                            "map {address:#x}, length {:#x}, origin base {:#x}, permissions {:#x}, offset {offset:#x}: {e}",
                            extent.len, extent.vm_start, extent.perms
                        ))
                    })?;
            }
            core::mem::swap(&mut self.installed, desired);
            parked
                .finish_after_mapping_ack(MappingOutcome::Committed)
                .unwrap();
        }
        let id = task.get_id();
        let generation = scheduler::get_task_pool()
            .task_generation(id)
            .expect("Running Task") as u64;
        let regs = load_registers(task, tf);
        let task_id = TaskId { id, generation };
        if endpoint.as_ref().is_none_or(|(id, _)| *id != self.id) {
            *endpoint = Some((self.id, self.worker.open_thread(task_id)?));
        }
        let permit = self
            .gate
            .issue(task_id, cpu)
            .map_err(|_| io::Error::other("Worker cannot admit Task"))?;
        endpoint
            .as_mut()
            .unwrap()
            .1
            .issue(permit, &regs, tf.tpidr_el0)?;
        Ok(Running {
            permit,
            interrupts: self.interrupts.clone(),
        })
    }
}

/// Translate a VMA snapshot into the resident, permission-checked backing
/// that a hosted executor can publish while the guest is parked.
fn visit_resident_extents(
    m: &VirtualMemoryMap,
    mut visit: impl FnMut(usize, u64, usize, usize),
) {
    use crate::environment::PAGE_SIZE;
    use crate::object::capability::memory_mapping::{AccessKind, AccessOp};

    if let Some(owner) = &m.owner {
        let first = (m.vmarea.start - m.vm_start) / PAGE_SIZE;
        owner.visit_resident_extents(
            first,
            m.vmarea.size() / PAGE_SIZE,
            &mut |page, count, pa| {
                let address = m.vm_start + page * PAGE_SIZE;
                let read = AccessKind { op: AccessOp::Load, vaddr: address, size: Some(1) };
                // A private copy must be committed by the shared VM fault path
                // before any access that requires it can be published.
                if !m.is_shared && owner.private_fault_requires_copy(&read) {
                    return;
                }
                let mut perms = owner.fault_page_permissions(&read, m.permissions)
                    & m.maximum_permissions;
                let write = AccessKind { op: AccessOp::Store, ..read };
                if !m.is_shared && owner.private_fault_requires_copy(&write) {
                    perms &= !(Perm::Write as usize);
                }
                visit(address, pa, count * PAGE_SIZE, perms);
            },
        );
    } else if m.pmarea.start != 0 {
        visit(m.vmarea.start, m.pmarea.start, m.vmarea.size(), m.permissions);
    }
}

fn load_registers(task: &Task, tf: &Trapframe) -> Registers {
    let vcpu = task.vcpu.lock();
    Registers {
        x: tf.regs.reg.map(|r| r as u64),
        pc: tf.elr,
        sp: tf.sp,
        nzcv: tf.spsr as u32,
        q: vcpu.fpu.v.map(|[lo, hi]| lo as u128 | ((hi as u128) << 64)),
        fpcr: vcpu.fpu.fpcr as u32,
        fpsr: vcpu.fpu.fpsr as u32,
    }
}

fn store_stop(task: &Task, tf: &mut Trapframe, guest: &GuestStop) {
    let regs = guest.registers;
    tf.elr = regs.pc;
    tf.sp = regs.sp;
    tf.regs.reg = regs.x.map(|r| r as usize);
    tf.spsr = u64::from(regs.nzcv);
    tf.tpidr_el0 = guest.tls;
    tf.esr_el1 = guest.esr;
    tf.far_el1 = guest.far;
    stop_profile::record(task, tf, guest.interrupted);
    let mut vcpu = task.vcpu.lock();
    vcpu.fpu_used = true;
    vcpu.fpu.v = regs.q.map(|q| [q as u64, (q >> 64) as u64]);
    vcpu.fpu.fpcr = u64::from(regs.fpcr);
    vcpu.fpu.fpsr = u64::from(regs.fpsr);
}

enum GuestRun {
    Trap,
    Resume,
    #[cfg(target_os = "ios")]
    RetryLoader,
}

impl GuestRun {
    fn from_stop(stop: &GuestStop) -> Self {
        if stop.interrupted {
            Self::Resume
        } else {
            Self::Trap
        }
    }
}

pub fn enter(tf: &mut Trapframe) -> ! {
    crate::arch::interrupt::enable_interrupts();
    loop {
        super::interrupts::dispatch_before_guest_entry(tf);
        scheduler::process_pending_events_before_user_return(tf);
        {
            let task = crate::task::mytask().unwrap_or_else(|| {
                let cpu = crate::arch::get_cpu().get_cpuid();
                let id = scheduler::current_task_id(cpu);
                let owner = id.and_then(scheduler::get_task_by_id).map(|task| (task.get_id(),task.running_cpu.load(Ordering::SeqCst),task.get_state()));
                panic!("No owned user Task on vCPU {cpu}, current={id:?} owner={owner:?}, guest pc={:#x}",tf.elr)
            });
            let result = task.vm_manager.hosted_image().run(&task, tf);
            match result {
                Ok(GuestRun::Resume) => continue,
                Ok(GuestRun::Trap) => {}
                #[cfg(target_os = "ios")]
                Ok(GuestRun::RetryLoader) => {
                    // The execution lease has been dropped. The blocked stack
                    // must not own any mapping that teardown needs to reclaim.
                    tf.elr -= 4;
                    task.sleep(tf, 1_000_000);
                    continue;
                }
                Err(error) => {
                    crate::println!(
                        "[hosted] Task {} stopped at {:#x}: {}",
                        task.get_id(),
                        tf.elr,
                        error
                    );
                    task.exit_group(139);
                    scheduler::schedule(tf);
                    continue;
                }
            }
        }
        static TRACE: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("SCARLET_TRACE_USER_TRAPS").is_some());
        if *TRACE {
            std::eprintln!(
                "guest pid={} pc={:#x} esr={:#x} far={:#x} syscall={} args={:x?}",
                crate::task::mytask().unwrap().get_id(),
                tf.elr,
                tf.esr_el1,
                tf.far_el1,
                tf.get_syscall_number(),
                &tf.regs.reg[..6]
            );
        }
        crate::arch::trap::user::dispatch_user_trap(tf, 0);
        let cpu = crate::arch::get_cpu().get_cpuid();
        if scheduler::may_schedule_from_interrupt(cpu) && scheduler::take_deferred_reschedule(cpu) {
            scheduler::schedule(tf);
        }
    }
}
