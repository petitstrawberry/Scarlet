//! Logical vCPU binding and IRQ masking on host threads.

use crate::environment::MAX_NUM_CPUS;
use core::{
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

use scarlet_hosted_runtime::cpu_binding;

static CPU_OWNED: [AtomicBool; MAX_NUM_CPUS] = [const { AtomicBool::new(false) }; MAX_NUM_CPUS];

pub(crate) struct CpuBinding {
    id: usize,
    _thread: PhantomData<*mut ()>,
}

impl CpuBinding {
    pub(crate) fn bind(id: usize) -> Result<Self, &'static str> {
        let slot = CPU_OWNED.get(id).ok_or("Invalid logical CPU ID")?;
        if try_get_cpuid().is_some() {
            return Err("Host thread already owns a logical CPU");
        }
        slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Logical CPU is already bound")?;
        crate::arch::aarch64::init_host_cpu(id);
        cpu_binding::bind(id);
        Ok(Self {
            id,
            _thread: PhantomData,
        })
    }
}

impl Drop for CpuBinding {
    fn drop(&mut self) {
        assert_eq!(try_get_cpuid(), Some(self.id));
        assert!(crate::sync::preemptible(), "vCPU unbound with a guard held");
        cpu_binding::unbind();
        CPU_OWNED[self.id].store(false, Ordering::Release);
    }
}

pub fn try_get_cpuid() -> Option<usize> {
    cpu_binding::current()
}

pub fn are_interrupts_enabled() -> bool {
    cpu_binding::interrupts_enabled()
}

pub fn disable_interrupts() {
    cpu_binding::set_interrupts(false);
}

pub fn enable_interrupts() {
    cpu_binding::set_interrupts(true);
}

pub fn save_and_disable_interrupts() -> usize {
    usize::from(!cpu_binding::set_interrupts(false))
}

pub fn restore_interrupts(saved: usize) {
    // Events are consumed at explicit vCPU safe points.
    cpu_binding::set_interrupts(saved == 0);
}
