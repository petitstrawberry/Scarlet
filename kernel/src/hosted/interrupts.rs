//! CPU-local ingress. Host threads publish notifications and wake a vCPU.
//! Only the vCPU consumes them and runs Scarlet handlers.

use super::execution::InterruptTarget;
use alloc::{boxed::Box, sync::Weak};
use scarlet_hosted_runtime::run_gate::RunPermit;
use std::{
    sync::{Condvar, Mutex, OnceLock},
    time::Duration,
};

use crate::interrupt::{
    CpuId, InterruptError, InterruptId, InterruptManager, InterruptResult, Priority,
    controllers::{
        ExternalInterruptController, InterruptControllerInitMode, LocalInterruptType, PendingIrq,
        SoftwareInterruptController, TimerController,
    },
};

use crate::arch::timer::get_time;

const LINE_COUNT: InterruptId = 64;
static CPUS: OnceLock<Box<[Ingress]>> = OnceLock::new();

#[derive(Default)]
struct Ingress {
    state: Mutex<State>,
    idle_changed: Condvar,
    guest_changed: Condvar,
}

struct State {
    guest: Option<(RunPermit, Weak<InterruptTarget>)>,
    // Keep the IRQ helper armed across short syscalls. The idle vCPU takes
    // over its own deadline until the next guest entry.
    watch_guest: bool,
    deadline: Option<u64>,
    arm_generation: u64,
    timer_enabled: bool,
    ipi_enabled: bool,
    ipi_pending: bool,
    threshold: Priority,
    lines: [Line; LINE_COUNT as usize],
}

#[derive(Clone, Copy)]
struct Line {
    priority: Priority,
    enabled: bool,
    pending: bool,
    active: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            guest: None,
            watch_guest: false,
            deadline: None,
            arm_generation: 0,
            timer_enabled: false,
            ipi_enabled: false,
            ipi_pending: false,
            threshold: 0,
            lines: [Line {
                priority: 1,
                enabled: false,
                pending: false,
                active: false,
            }; LINE_COUNT as usize],
        }
    }
}

impl State {
    fn set_deadline(&mut self, deadline: Option<u64>) {
        self.deadline = deadline;
        self.arm_generation = self.arm_generation.wrapping_add(1);
    }

    fn timer_due(&self, now: u64) -> bool {
        self.timer_enabled && self.deadline.is_some_and(|deadline| deadline <= now)
    }

    fn ipi_ready(&self) -> bool {
        self.ipi_enabled && self.ipi_pending
    }

    fn eligible(&self, line: &Line) -> bool {
        line.enabled && line.pending && !line.active && line.priority > self.threshold
    }

    fn has_external(&self) -> bool {
        self.lines.iter().any(|line| self.eligible(line))
    }
}

struct Controller;

fn cpu(id: CpuId) -> InterruptResult<&'static Ingress> {
    CPUS.get()
        .and_then(|cpus| cpus.get(id as usize))
        .ok_or(InterruptError::InvalidCpuId)
}

impl Ingress {
    fn update(&self, operation: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap();
        operation(&mut state);
        let waiter = if state.watch_guest {
            &self.guest_changed
        } else {
            &self.idle_changed
        };
        drop(state);
        waiter.notify_one();
    }
}

pub(super) fn initialize(count: usize) -> Result<(), &'static str> {
    CPUS.set((0..count).map(|_| Ingress::default()).collect())
        .map_err(|_| "Hosted interrupt ingress is already initialized")?;
    let manager = InterruptManager::global();
    manager
        .register_timer_controller_for_range(Box::new(LocalTimer), 0..count as CpuId)
        .map_err(|_| "Cannot register the hosted timer controller")?;
    manager
        .register_software_interrupt_controller_for_range(Box::new(LocalIpi), 0..count as CpuId)
        .map_err(|_| "Cannot register the hosted software interrupt controller")?;
    manager
        .register_external_controller(Box::new(Controller))
        .map_err(|_| "Cannot register the hosted external controller")?;
    for id in 0..count {
        std::thread::Builder::new()
            .name(alloc::format!("scarlet-irq-{id}"))
            .spawn(move || {
                let ingress = cpu(id as CpuId).unwrap();
                let mut state = ingress.state.lock().unwrap();
                loop {
                    let now = get_time();
                    if (state.timer_due(now) || state.ipi_ready() || state.has_external())
                        && let Some((permit, target)) = state.guest.take()
                    {
                        drop(state);
                        if let Some(target) = target.upgrade() {
                            target.request(permit);
                        }
                        state = ingress.state.lock().unwrap();
                        continue;
                    }
                    // Keep tracking the deadline across syscalls. During idle
                    // the vCPU waits on the deadline itself.
                    if let Some(deadline) = state
                        .deadline
                        .filter(|&t| state.watch_guest && state.timer_enabled && t > now)
                    {
                        state = ingress
                            .guest_changed
                            .wait_timeout(state, Duration::from_nanos(deadline - now))
                            .unwrap()
                            .0;
                        continue;
                    }
                    state = ingress.guest_changed.wait(state).unwrap();
                }
            })
            .map_err(|_| "Cannot start hosted IRQ delivery")?;
    }
    Ok(())
}

pub(super) fn guest_entered(permit: RunPermit, target: Weak<InterruptTarget>) {
    let ingress = cpu(permit.vcpu as CpuId).unwrap();
    let mut state = ingress.state.lock().unwrap();
    let resume_watch = !state.watch_guest;
    state.watch_guest = true;
    state.guest = Some((permit, target));
    if (resume_watch && state.timer_enabled && state.deadline.is_some())
        || state.timer_due(get_time())
        || state.ipi_ready()
        || state.has_external()
    {
        drop(state);
        ingress.guest_changed.notify_one();
    }
}

pub(super) fn guest_stopped(permit: RunPermit) {
    let mut state = cpu(permit.vcpu as CpuId).unwrap().state.lock().unwrap();
    if state
        .guest
        .as_ref()
        .is_some_and(|(active, _)| *active == permit)
    {
        state.guest = None;
    }
}

pub(crate) fn program_timer(cpu_id: usize, deadline: Option<u64>) {
    if let Ok(ingress) = cpu(cpu_id as CpuId) {
        ingress.update(|state| {
            state.set_deadline(deadline);
            state.timer_enabled = deadline.is_some();
        });
    }
}

pub fn raise_external(cpu_id: CpuId, id: InterruptId) -> InterruptResult<()> {
    let index = line_index(id)?;
    cpu(cpu_id)?.update(|state| state.lines[index].pending = true);
    Ok(())
}

fn line_index(id: InterruptId) -> InterruptResult<usize> {
    if id < LINE_COUNT {
        Ok(id as usize)
    } else {
        Err(InterruptError::InvalidInterruptId)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Pending {
    pub timer: bool,
    pub ipi: bool,
    pub external: bool,
}

/// Wait on the bound vCPU's current interrupt sources. Acknowledge timer
/// expiry under the same lock that arms the timer.
pub(super) fn wait(cpu_id: CpuId) -> Pending {
    let ingress = cpu(cpu_id).expect("Missing vCPU interrupt ingress");
    let mut state = ingress.state.lock().unwrap();
    state.watch_guest = false;
    loop {
        let now = get_time();
        let pending = Pending {
            timer: state.timer_due(now),
            ipi: state.ipi_ready(),
            external: state.has_external(),
        };
        if pending.timer || pending.ipi || pending.external {
            if pending.timer {
                state.set_deadline(None);
            }
            return pending;
        }
        if let Some(deadline) = state.deadline.filter(|_| state.timer_enabled) {
            let generation = state.arm_generation;
            let (next, _) = ingress
                .idle_changed
                .wait_timeout_while(
                    state,
                    Duration::from_nanos(deadline.saturating_sub(now)),
                    |state| {
                        state.arm_generation == generation
                            && state.timer_enabled
                            && !state.ipi_ready()
                            && !state.has_external()
                    },
                )
                .unwrap();
            state = next;
        } else {
            state = ingress.idle_changed.wait(state).unwrap();
        }
    }
}

fn dispatch(cpu_id: usize, pending: Pending) {
    use crate::{sched::scheduler, timer};
    if pending.ipi {
        LocalIpi
            .clear_software_interrupt(cpu_id as CpuId)
            .expect("Idle vCPU has an IPI controller");
        scheduler::acknowledge_reschedule_ipi(cpu_id);
        scheduler::defer_reschedule(cpu_id);
    }
    if pending.external {
        let result =
            InterruptManager::global().claim_and_handle_pending_external_interrupt(cpu_id as CpuId);
        if let Err(error) = result {
            crate::println!(
                "[hosted] external interrupt failed on CPU {}: {}",
                cpu_id,
                error
            );
        }
    }
    if pending.timer {
        timer::handle_local_timer_irq();
    }
}

pub(super) fn dispatch_before_guest_entry(tf: &mut crate::arch::Trapframe) {
    use crate::{arch::interrupt, sched::scheduler};
    let id = super::cpu::try_get_cpuid().unwrap();
    let mut state = cpu(id as CpuId).unwrap().state.lock().unwrap();
    let pending = Pending {
        timer: state.timer_due(get_time()),
        ipi: state.ipi_ready(),
        external: state.has_external(),
    };
    if !pending.timer && !pending.ipi && !pending.external {
        return;
    }
    if pending.timer {
        state.set_deadline(None);
    }
    drop(state);
    let mask = interrupt::save_and_disable_interrupts();
    dispatch(id, pending);
    scheduler::handle_timer_reschedule(id, tf, true);
    interrupt::restore_interrupts(mask);
}

pub fn idle() -> ! {
    use crate::{
        arch::{Trapframe, interrupt},
        sched::scheduler,
    };

    let cpu_id = super::cpu::try_get_cpuid().expect("Idle thread needs a bound vCPU");
    loop {
        interrupt::enable_interrupts();
        let pending = wait(cpu_id as CpuId);
        let mask = interrupt::save_and_disable_interrupts();
        dispatch(cpu_id, pending);
        let mut trapframe = Trapframe::new();
        // On this path the worker is parked and the CPU is running a
        // Scarlet kernel task. Preserve the task's saved guest state.
        if let Some(task) = scheduler::current_task(cpu_id) {
            task.vcpu.lock().switch(&mut trapframe);
        }
        scheduler::handle_timer_reschedule(cpu_id, &mut trapframe, true);
        interrupt::restore_interrupts(mask);
    }
}

struct LocalTimer;

impl TimerController for LocalTimer {
    fn init(&mut self, id: CpuId, _: InterruptControllerInitMode) -> InterruptResult<()> {
        cpu(id)?.update(|state| {
            state.timer_enabled = false;
            state.set_deadline(None);
        });
        Ok(())
    }

    fn enable_timer(&self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.timer_enabled = true);
        Ok(())
    }

    fn disable_timer(&self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.timer_enabled = false);
        Ok(())
    }

    fn is_timer_pending(&self, id: CpuId) -> bool {
        cpu(id).is_ok_and(|ingress| ingress.state.lock().unwrap().timer_due(get_time()))
    }

    fn clear_timer(&mut self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.set_deadline(None));
        Ok(())
    }

    fn set_timer(&self, id: CpuId, time: u64) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.set_deadline(Some(time)));
        Ok(())
    }

    fn get_time(&self) -> u64 {
        get_time()
    }

    fn get_timer_frequency_hz(&self) -> u64 {
        1_000_000_000
    }
}

struct LocalIpi;

impl SoftwareInterruptController for LocalIpi {
    fn init(&mut self, id: CpuId, _: InterruptControllerInitMode) -> InterruptResult<()> {
        cpu(id)?.update(|state| {
            state.ipi_enabled = false;
            state.ipi_pending = false;
        });
        Ok(())
    }

    fn enable_software_interrupt(&self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.ipi_enabled = true);
        Ok(())
    }

    fn disable_software_interrupt(&self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.ipi_enabled = false);
        Ok(())
    }

    fn is_software_interrupt_pending(&self, id: CpuId) -> bool {
        cpu(id).is_ok_and(|ingress| ingress.state.lock().unwrap().ipi_pending)
    }

    fn clear_software_interrupt(&mut self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.ipi_pending = false);
        Ok(())
    }

    fn send_software_interrupt(&self, id: CpuId) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.ipi_pending = true);
        Ok(())
    }
}

impl ExternalInterruptController for Controller {
    fn init(&mut self, _: InterruptControllerInitMode) -> InterruptResult<()> {
        for ingress in CPUS.get().unwrap() {
            ingress.update(|state| {
                state.lines = State::default().lines;
                state.threshold = 0;
            });
        }
        Ok(())
    }

    fn enable_interrupt(&self, id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        let index = line_index(id)?;
        cpu(cpu_id)?.update(|state| state.lines[index].enabled = true);
        Ok(())
    }

    fn disable_interrupt(&self, id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        let index = line_index(id)?;
        cpu(cpu_id)?.update(|state| state.lines[index].enabled = false);
        Ok(())
    }

    fn set_priority(&mut self, id: InterruptId, priority: Priority) -> InterruptResult<()> {
        let index = line_index(id)?;
        for ingress in CPUS.get().unwrap() {
            ingress.update(|state| state.lines[index].priority = priority);
        }
        Ok(())
    }

    fn get_priority(&self, id: InterruptId) -> InterruptResult<Priority> {
        let index = line_index(id)?;
        Ok(cpu(0)?.state.lock().unwrap().lines[index].priority)
    }

    fn set_threshold(&mut self, id: CpuId, threshold: Priority) -> InterruptResult<()> {
        cpu(id)?.update(|state| state.threshold = threshold);
        Ok(())
    }

    fn get_threshold(&self, id: CpuId) -> InterruptResult<Priority> {
        Ok(cpu(id)?.state.lock().unwrap().threshold)
    }

    fn claim_interrupt(&self, id: CpuId) -> InterruptResult<Option<InterruptId>> {
        let mut state = cpu(id)?.state.lock().unwrap();
        let candidate = state
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| state.eligible(line))
            .max_by_key(|&(index, line)| (line.priority, core::cmp::Reverse(index)))
            .map(|(index, _)| index);
        Ok(candidate.map(|index| {
            let line = &mut state.lines[index];
            line.pending = false;
            line.active = true;
            index as InterruptId
        }))
    }

    fn complete_interrupt(&self, cpu_id: CpuId, id: InterruptId) -> InterruptResult<()> {
        let index = line_index(id)?;
        cpu(cpu_id)?.update(|state| state.lines[index].active = false);
        Ok(())
    }

    fn is_pending(&self, id: InterruptId) -> bool {
        line_index(id).is_ok_and(|index| {
            CPUS.get()
                .unwrap()
                .iter()
                .any(|ingress| ingress.state.lock().unwrap().lines[index].pending)
        })
    }

    fn max_interrupts(&self) -> InterruptId {
        LINE_COUNT
    }

    fn max_cpus(&self) -> CpuId {
        CPUS.get().unwrap().len() as CpuId
    }

    fn mask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.disable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    fn unmask_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.enable_interrupt(irq.mapping.hwirq, irq.cpu_id)
    }

    fn send_ipi(&self, id: CpuId, kind: LocalInterruptType) -> InterruptResult<()> {
        if !matches!(kind, LocalInterruptType::Software) {
            return Err(InterruptError::NotSupported);
        }
        LocalIpi.send_software_interrupt(id)
    }
}
