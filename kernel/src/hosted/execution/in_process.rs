//! One carrier for every signed image on hosted iOS.

use super::{Image, load_registers, store_stop};
use crate::{arch::Trapframe, sched::scheduler, task::Task, vm::manager::VirtualMemoryManager};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use scarlet_hosted_runtime::{
    in_process::{Carrier, SignedText},
    run_gate::{MappingOutcome, RunGate, RunPermit, TaskId, WorkerId},
};
use std::{io, sync::OnceLock};

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
    text: Arc<SignedText>,
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
    install_mapping(&task.vm_manager, text)
}

fn install_mapping(vm: &VirtualMemoryManager, text: Arc<SignedText>) -> io::Result<()> {
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
        text,
        gate: RunGate::new(id),
        interrupts: Arc::new(InterruptTarget { carrier }),
    }));
    Ok(())
}

impl Image {
    pub(super) fn fork_signed_into(&self, child: &VirtualMemoryManager) -> Result<(), &'static str> {
        let local = self.local.lock().unwrap();
        let Some(local) = local.as_ref() else { return Ok(()) };
        // The caller owns the parent's mapping transaction and the child is unpublished.
        let text = unsafe { local.text.fork() }.map_err(|error| {
            std::eprintln!("Cannot fork signed guest pages: {error}");
            "Cannot fork signed guest pages"
        })?;
        install_mapping(child, text).map_err(|_| "Cannot attach forked signed image")
    }

    pub(crate) fn signed_alias(&self, address: usize, write: bool) -> Option<Option<usize>> {
        let local = self.local.lock().unwrap();
        let text = &local.as_ref()?.text;
        Some(
            text.user_bounds(address, write)
                .map(|_| text.kernel_alias(address)),
        )
    }

    pub(crate) fn has_signed_mapping(&self) -> bool {
        self.local.lock().unwrap().is_some()
    }

    /// The VM transaction and its stop acknowledgement protect the lease.
    pub(crate) unsafe fn resize_heap(
        &self,
        address: usize,
        limit: usize,
    ) -> Result<usize, &'static str> {
        let image = self.local.lock().unwrap();
        let image = image.as_ref().ok_or("Signed heap is absent")?;
        unsafe { image.text.resize_heap(address, limit) }
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
        Some(image.as_ref()?.text.user_bounds(address, write))
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

    pub(super) fn run(self: &Arc<Self>, task: &Task, tf: &mut Trapframe) -> io::Result<bool> {
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
            // Under iOS UP the previous carrier request has acknowledged its stop.
            unsafe { image.text.activate()? };
            let permit = image
                .gate
                .issue(TaskId { id, generation }, 0)
                .map_err(|_| io::Error::other("iOS image cannot admit Task"))?;
            let result = image.interrupts.carrier.issue(
                permit,
                image.text.clone(),
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
        Ok(guest.interrupted)
    }
}
