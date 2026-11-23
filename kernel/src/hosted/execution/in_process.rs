//! One carrier for every signed image on hosted iOS.

mod data;
mod link;

use super::{GuestRun, Image, load_registers, store_stop};
use crate::{arch::Trapframe, sched::scheduler, task::Task, vm::manager::VirtualMemoryManager};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use data::DataWindow;
use scarlet_hosted_runtime::{
    in_process::{Carrier, SignedText},
    run_gate::{MappingOutcome, RunGate, RunPermit, TaskId, WorkerId},
};
use std::{
    io,
    sync::{Mutex, OnceLock},
};

static CARRIER: OnceLock<Carrier> = OnceLock::new();

pub(in crate::hosted) fn prepare() -> Result<(), &'static str> {
    let carrier = Carrier::start().map_err(|error| {
        std::eprintln!("Cannot start iOS guest carrier: {error}");
        "Cannot start iOS guest carrier"
    })?;
    CARRIER
        .set(carrier)
        .map_err(|_| "iOS guest carrier already installed")
}

pub(super) struct LocalImage {
    text: Mutex<Arc<SignedText>>,
    link: Mutex<link::LinkState>,
    data: Arc<DataWindow>,
    generation: u64,
    gate: RunGate,
    interrupts: Arc<InterruptTarget>,
}

pub(in crate::hosted) struct InterruptTarget {
    carrier: &'static Carrier,
}

impl InterruptTarget {
    pub(in crate::hosted) fn request(&self, permit: RunPermit) {
        if let Err(error) = self.carrier.interrupt(permit) {
            // A failed notification cannot establish that this address space
            // is parked. The host must stop before its mappings can be reused.
            std::eprintln!("Cannot stop iOS guest carrier: {error}");
            std::process::abort();
        }
    }
}

/// Admit an already registered signed image before publishing its first Task.
/// The loader owns all of its mappings, including the writable stack.
pub fn install_signed_text(task: &Task, text: Arc<SignedText>) -> io::Result<()> {
    let data = Arc::new(DataWindow::new()?);
    task.vm_manager
        .set_mmap_window(data.range())
        .map_err(io::Error::other)?;
    install_mapping(&task.vm_manager, text, data, link::LinkState::default())
}

fn install_mapping(
    vm: &VirtualMemoryManager,
    text: Arc<SignedText>,
    data: Arc<DataWindow>,
    link: link::LinkState,
) -> io::Result<()> {
    let carrier = CARRIER
        .get()
        .ok_or_else(|| io::Error::other("iOS guest carrier unavailable"))?;
    let _transaction = vm.mapping_transaction();
    let image = vm.hosted_image();
    let mut local = image.local.lock().unwrap();
    if local.is_some() {
        return Err(io::Error::other("Address space already has signed text"));
    }
    static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
    let generation = NEXT_GENERATION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| io::Error::other("Image generation exhausted"))?;
    let id = WorkerId {
        address_space: vm.get_asid() as usize,
        image_generation: generation,
    };
    *local = Some(Arc::new(LocalImage {
        text: Mutex::new(text),
        link: Mutex::new(link),
        data,
        generation,
        gate: RunGate::new(id),
        interrupts: Arc::new(InterruptTarget { carrier }),
    }));
    Ok(())
}

impl Image {
    pub(super) fn fork_signed_into(
        &self,
        child: &VirtualMemoryManager,
    ) -> Result<(), &'static str> {
        let local = self.local.lock().unwrap();
        let Some(local) = local.as_ref() else {
            return Ok(());
        };
        // The caller owns the parent's mapping transaction and the child is unpublished.
        let text = unsafe { local.text.lock().unwrap().fork() }.map_err(|error| {
            std::eprintln!("Cannot fork signed guest pages: {error}");
            "Cannot fork signed guest pages"
        })?;
        let parent = crate::task::mytask().ok_or("Signed fork has no current task")?;
        let link = local.link.lock().unwrap().fork(&parent);
        install_mapping(child, text, local.data.clone(), link)
            .map_err(|_| "Cannot attach forked signed image")
    }

    pub(crate) fn signed_alias(&self, address: usize, write: bool) -> Option<Option<usize>> {
        let local = self.local.lock().unwrap();
        let image = local.as_ref()?;
        if image.data.contains(address) {
            return None;
        }
        let text = image.text.lock().unwrap();
        Some(
            text.user_bounds(address, write)
                .map(|_| text.kernel_alias(address)),
        )
    }

    pub(crate) fn has_signed_mapping(&self) -> bool {
        self.local.lock().unwrap().is_some()
    }

    pub(crate) fn signed_instruction(&self, address: usize) -> Option<bool> {
        let local = self.local.lock().unwrap();
        let image = local.as_ref()?;
        Some(image.text.lock().unwrap().contains_guest_instruction(address as u64))
    }

    pub(crate) fn signed_text_size(&self) -> Option<usize> {
        let local = self.local.lock().unwrap();
        Some(local.as_ref()?.text.lock().unwrap().committed_text_size())
    }

    /// The VM transaction and its stop acknowledgement protect the lease.
    pub(crate) unsafe fn resize_heap(
        &self,
        address: usize,
        limit: usize,
    ) -> Result<usize, &'static str> {
        let image = self.local.lock().unwrap();
        let image = image.as_ref().ok_or("Signed heap is absent")?;
        unsafe { image.text.lock().unwrap().resize_heap(address, limit) }
            .map_err(|_| "Cannot resize signed guest heap")
    }

    /// Some means this address space has signed text. A miss must stay a
    /// miss, even if the legacy software map covers a numeric address.
    pub(crate) fn signed_bounds(
        &self,
        address: usize,
        write: bool,
    ) -> Option<Option<core::ops::Range<usize>>> {
        let image = self.local.lock().unwrap();
        let image = image.as_ref()?;
        if image.data.contains(address) {
            return None;
        }
        Some(image.text.lock().unwrap().user_bounds(address, write))
    }

    pub(crate) fn try_parked(&self) -> bool {
        self.local
            .try_lock()
            .is_ok_and(|local| local.as_ref().is_none_or(|image| image.gate.try_parked()))
    }

    pub(crate) fn quiesce(&self) {
        let local = self.local.lock().unwrap().clone();
        let Some(image) = local else { return };
        if image.gate.try_parked() {
            return;
        }
        let barrier = image
            .gate
            .close_for_mapping()
            .expect("Concurrent iOS admission barrier");
        for &permit in barrier.issued_permits() {
            image.interrupts.request(permit);
        }
        barrier
            .wait()
            .expect("iOS image retired during quiescence")
            .finish_after_mapping_ack(MappingOutcome::Restored)
            .unwrap();
    }

    pub(super) fn run(self: &Arc<Self>, task: &Task, tf: &mut Trapframe) -> io::Result<GuestRun> {
        let (image, permit) = {
            let _transaction = task.vm_manager.mapping_transaction();
            let image = self
                .local
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| io::Error::other("Task has no signed iOS image"))?;
            let id = task.get_id();
            let generation = scheduler::get_task_pool()
                .task_generation(id)
                .expect("Running Task") as u64;
            let thread = TaskId { id, generation };
            // Under iOS UP the previous carrier request has acknowledged its stop.
            let text = image.text.lock().unwrap().clone();
            unsafe { text.activate()? };
            image.data.publish(
                &task.vm_manager,
                image.generation,
                self.publication_generation.load(Ordering::Acquire),
            )?;
            if id == task.get_thread_group_id() {
                image.link.lock().unwrap().bind_fork_owner(thread);
            }
            let permit = image
                .gate
                .issue(thread, 0)
                .map_err(|_| io::Error::other("iOS image cannot admit Task"))?;
            let result = image.interrupts.carrier.issue(
                permit,
                text,
                load_registers(task, tf),
                tf.tpidr_el0,
            );
            if let Err(error) = result {
                assert!(image.gate.acknowledge_stop(permit));
                return Err(error);
            }
            (image, permit)
        };
        super::super::interrupts::guest_entered(permit, Arc::downgrade(&image.interrupts));
        let result = image.interrupts.carrier.receive(permit);
        assert!(image.gate.acknowledge_stop(permit));
        super::super::interrupts::guest_stopped(permit);
        let guest = result?;
        store_stop(task, tf, &guest);
        if let Some(operation) = guest.link {
            // Usercopy and VFS may sleep. Group teardown drains this bounded
            // operation before retiring its stack, but never waits for guest
            // constructors or a competing loader operation through the guard.
            let Ok(transaction) = crate::task::kernel_transaction() else {
                tf.regs.reg[0] = link::failure_return(operation);
                tf.regs.reg[1] = 0;
                return Ok(GuestRun::Resume);
            };
            let outcome = link::dispatch(&image, task, permit.task, tf, operation);
            drop(image);
            drop(transaction);
            return Ok(outcome);
        }
        Ok(GuestRun::from_stop(&guest))
    }
}
