//! One carrier for every signed image on hosted iOS.

use super::{Image, load_registers, store_stop};
use crate::{arch::Trapframe, sched::scheduler, task::Task};
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
    let carrier = CARRIER
        .get()
        .ok_or_else(|| io::Error::other("iOS guest carrier unavailable"))?;
    let _transaction = task.vm_manager.mapping_transaction();
    let image = task.vm_manager.hosted_image();
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
        address_space: task.vm_manager.get_asid() as usize,
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
