//! Thread-local guest transitions through the host signal context.

use crate::{
    fetched_instruction::{self, Fetch, Slots},
    worker_context::{Exception, Registers},
    worker_memory::{Mapping, PatchedInstruction},
};
use std::{
    collections::BTreeMap,
    ffi::c_void,
    io, ptr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(all(test, target_os = "macos"))]
mod linked_tests;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::handle_signal;

#[cfg(target_os = "macos")]
use crate::platform_register::Action;

const STACK_SIZE: usize = 128 * 1024;

pub struct GuestStop {
    pub registers: Registers,
    pub esr: u64,
    pub far: u64,
    pub tls: u64,
    pub interrupted: bool,
}

/// The coordinator can request an epoch before its RUN stream has been read.
#[derive(Default)]
pub(super) struct Interrupt {
    epoch: AtomicU64,
    // The owner clears the pthread handle before tearing down its alt stack.
    thread: Mutex<Option<usize>>,
}

impl Interrupt {
    pub fn request(&self, epoch: u64) -> io::Result<()> {
        let thread = self.thread.lock().unwrap();
        if epoch > self.epoch.fetch_max(epoch, Ordering::SeqCst) {
            if let Some(handle) = *thread {
                let status =
                    unsafe { libc::pthread_kill(handle as libc::pthread_t, libc::SIGUSR1) };
                if status != 0 {
                    return Err(io::Error::from_raw_os_error(status));
                }
            }
        }
        Ok(())
    }
}

struct Control {
    crossings: fetched_instruction::linked::Crossings,
    interrupt: Arc<Interrupt>,
    epoch: u64,
    interrupted: bool,
    // Borrowed only between the entry BRK and the return to the host.
    mappings: *const BTreeMap<usize, Mapping>,
    #[cfg(target_os = "macos")]
    x18: u64,
    tls: u64,
    #[cfg(target_os = "macos")]
    step: Option<NativeStep>,
    pending: Option<Registers>,
    host: Option<Registers>,
    stop: Option<Exception>,
}

#[cfg(target_os = "macos")]
struct NativeStep {
    guest_pc: u64,
    step_pc: u64,
    exit: u8,
    scratch: usize,
    saved: u64,
    saved_high: Option<u64>,
}

#[cfg(target_os = "macos")]
impl NativeStep {
    // The sealed code has the same instruction offsets as the guest and ends
    // at a private BRK. Restore the faulting or interrupted guest boundary.
    fn finish(self, state: &mut libc::__darwin_arm_thread_state64, x18: &mut u64) -> bool {
        let offset = state.__pc.wrapping_sub(self.step_pc);
        let completed = offset == u64::from(self.exit) * 4;
        // A loop carries virtual x18 in scratch at every instruction boundary.
        if completed || self.exit > 1 {
            *x18 = state.__x[self.scratch];
        }
        if let Some(original) = self.saved_high {
            if completed {
                state.__x[19] = state.__x[self.scratch + 1];
            }
            state.__x[self.scratch + 1] = original;
        }
        state.__x[self.scratch] = self.saved;
        state.__pc = if self.exit == 1 && !completed {
            self.guest_pc
        } else {
            self.guest_pc.wrapping_add(offset)
        };
        completed
    }
}

#[cfg(target_os = "macos")]
impl Control {
    fn read_register(&self, state: &libc::__darwin_arm_thread_state64, register: u8) -> u64 {
        match register {
            18 => self.x18,
            29 => state.__fp,
            30 => state.__lr,
            31 => 0,
            n => state.__x[n as usize],
        }
    }

    fn write_register(&mut self, state: &mut libc::__darwin_arm_thread_state64, register: u8, value: u64) {
        match register {
            18 => self.x18 = value,
            29 => state.__fp = value,
            30 => state.__lr = value,
            31 => {}
            n => state.__x[n as usize] = value,
        }
    }

    fn platform_trap(
        &mut self,
        mapping: &Mapping,
        action: Action,
        state: &mut libc::__darwin_arm_thread_state64,
    ) -> bool {
        let pc = state.__pc;
        match action {
            Action::Move {
                source,
                destination,
                wide,
            } => {
                let value = self.read_register(state, source);
                let value = if wide { value } else { u64::from(value as u32) };
                self.write_register(state, destination, value);
                state.__pc += 4;
            }
            Action::Address { offset, page } => {
                let base = if page { pc & !4095 } else { pc };
                self.x18 = base.wrapping_add_signed(offset);
                state.__pc += 4;
            }
            Action::Jump { link } => {
                if link {
                    state.__lr = pc + 4;
                }
                state.__pc = self.x18;
            }
            Action::Test {
                mask,
                nonzero,
                offset,
            } => {
                state.__pc = if (self.x18 & mask != 0) == nonzero {
                    pc.wrapping_add_signed(offset)
                } else {
                    pc + 4
                };
            }
            Action::Native {
                slot,
                scratch,
                literal,
                paired,
            } => {
                let step_pc = mapping.step_address(slot);
                let scratch = usize::from(scratch);
                self.step = Some(NativeStep {
                    guest_pc: pc,
                    step_pc,
                    exit: 1,
                    scratch,
                    saved: state.__x[scratch],
                    saved_high: paired.then(|| {
                        let high = state.__x[19];
                        std::mem::replace(&mut state.__x[scratch + 1], high)
                    }),
                });
                state.__x[scratch] =
                    literal.map_or(self.x18, |offset| pc.wrapping_add_signed(offset));
                state.__pc = step_pc;
            }
            Action::NativeLoop {
                slot,
                entry,
                exit,
                scratch,
            } => {
                let step_pc = mapping.step_address(slot);
                let entry_offset = 4 * u64::from(entry);
                let scratch = usize::from(scratch);
                self.step = Some(NativeStep {
                    guest_pc: pc - entry_offset,
                    step_pc,
                    exit,
                    scratch,
                    saved: std::mem::replace(&mut state.__x[scratch], self.x18),
                    saved_high: None,
                });
                state.__pc = step_pc + entry_offset;
            }
            Action::Unsupported => return false,
        }
        true
    }
}

pub struct Thread {
    mapping: *mut c_void,
    control: *mut Control,
    previous_stack: libc::stack_t,
    slots: Slots,
    pub(super) exclusive_initial: Option<Registers>,
    pub(super) retry: Option<u64>,
}

pub(super) fn mask_interrupt(how: i32) -> io::Result<()> {
    let mut mask = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGUSR1);
        let status = libc::pthread_sigmask(how, &mask, ptr::null_mut());
        if status == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status))
        }
    }
}

impl Thread {
    pub(super) fn new(interrupt: Arc<Interrupt>) -> io::Result<Self> {
        static SIGNALS: OnceLock<Result<(), i32>> = OnceLock::new();
        SIGNALS
            .get_or_init(|| {
                install_actions().map_err(|error| error.raw_os_error().unwrap_or(libc::EINVAL))
            })
            .map_err(io::Error::from_raw_os_error)?;
        let mapping = signal_stack()?;
        let control = mapping.cast::<Control>();
        let header = size_of::<Control>().next_multiple_of(16);
        let stack = libc::stack_t {
            ss_sp: unsafe { mapping.byte_add(header) },
            ss_size: STACK_SIZE - header,
            ss_flags: 0,
        };
        let mut previous_stack = std::mem::MaybeUninit::uninit();
        unsafe {
            control.write(Control {
                crossings: Default::default(),
                interrupt: interrupt.clone(),
                epoch: 0,
                interrupted: false,
                mappings: ptr::null(),
                #[cfg(target_os = "macos")]
                x18: 0,
                tls: 0,
                #[cfg(target_os = "macos")]
                step: None,
                pending: None,
                host: None,
                stop: None,
            });
            if libc::sigaltstack(&stack, previous_stack.as_mut_ptr()) != 0 {
                let error = io::Error::last_os_error();
                control.drop_in_place();
                libc::munmap(mapping, STACK_SIZE);
                return Err(error);
            }
            if let Err(error) = mask_interrupt(libc::SIG_UNBLOCK) {
                libc::sigaltstack(previous_stack.as_ptr(), ptr::null_mut());
                control.drop_in_place();
                libc::munmap(mapping, STACK_SIZE);
                return Err(error);
            }
            *interrupt.thread.lock().unwrap() = Some(libc::pthread_self() as usize);
            Ok(Self {
                mapping,
                control,
                previous_stack: previous_stack.assume_init(),
                slots: Slots::default(),
                exclusive_initial: None,
                retry: None,
            })
        }
    }

    fn enter(
        &mut self,
        epoch: u64,
        registers: Registers,
        tls: &mut u64,
        mappings: &BTreeMap<usize, Mapping>,
    ) -> Exception {
        unsafe {
            (*self.control).epoch = epoch;
            (*self.control).interrupted = false;
            (*self.control).mappings = mappings;
            (*self.control).tls = *tls;
            (*self.control).pending = Some(registers);
            core::arch::asm!("brk #0x5600");
            (*self.control).mappings = ptr::null();
            *tls = (*self.control).tls;
            (*self.control)
                .stop
                .take()
                .expect("Worker returned without a stop")
        }
    }

    fn fetched_step(
        &mut self,
        epoch: u64,
        registers: &mut Registers,
        tls: &mut u64,
        mappings: &BTreeMap<usize, Mapping>,
        word: u32,
    ) -> io::Result<Option<GuestStop>> {
        let pc = registers.pc;
        let stop = |registers, tls, esr, far, interrupted| {
            Ok(Some(GuestStop {
                registers,
                tls,
                esr,
                far,
                interrupted,
            }))
        };
        // A loop of interpreted branches need never enter a signal context.
        if unsafe { (&(*self.control).interrupt).epoch.load(Ordering::SeqCst) } >= epoch {
            return stop(*registers, *tls, 0, 0, true);
        }
        let action = fetched_instruction::lower(word, registers, tls, |pc| {
            let pc = pc as usize;
            mappings.range(..=pc).next_back()?.1.published_word(pc)
        });
        let interval = |start| mappings.range(..=start).next_back()?.1.fetch_words(start);
        let action = match action {
            Fetch::Advanced => {
                let block = fetched_instruction::taken_loop(word, pc, registers.pc, |start, count| {
                    interval(start)?.get(..count)
                });
                let Some(block) = block else { return Ok(None) };
                Fetch::Block(block)
            }
            Fetch::Native { .. } => interval(pc as usize)
                .and_then(|words| fetched_instruction::straight_line(pc, words))
                .map_or(action, Fetch::Block),
            other => other,
        };
        let (word, scratch, writes_x18, paired, svc) = match action {
                Fetch::Advanced => unreachable!(),
                Fetch::Undefined => return stop(*registers, *tls, 1 << 25, 0, false),
                Fetch::Block(block) => {
                    let initial = *registers;
                    let slot = self.slots.address(&block.words)?;
                    if let Some(scratch) = block.scratch {
                        registers.x[scratch] = initial.x[18];
                    }
                    registers.pc = slot;
                    let exception = self.enter(epoch, *registers, tls, mappings);
                    *registers = exception.registers;
                    if let Some(scratch) = block.scratch {
                        registers.x[18] = registers.x[scratch];
                        registers.x[scratch] = initial.x[scratch];
                    }
                    let index = registers
                        .pc
                        .checked_sub(slot)
                        .filter(|offset| offset % 4 == 0)
                        .and_then(|offset| usize::try_from(offset / 4).ok())
                        .filter(|&i| i < block.pcs.len())
                        .ok_or_else(|| {
                            io::Error::other("Guest block stopped outside its native slot")
                        })?;
                    registers.pc = block.pcs[index];
                    if unsafe { (*self.control).interrupted } {
                        return stop(*registers, *tls, 0, 0, true);
                    }
                    if block.words[index] == crate::platform_register::RETURN
                        && exception.esr >> 26 == 0x3c
                        && exception.esr & 0xffff == 0x5800
                    {
                        return Ok(None);
                    }
                    if exception.esr >> 26 == 0x24
                        && crate::exclusive::Access::decode(word).is_some_and(|a| a.load)
                    {
                        self.exclusive_initial = Some(initial);
                    }
                    return stop(*registers, *tls, exception.esr.into(), exception.far, false);
                }
                Fetch::Native {
                    word,
                    scratch,
                    writes_x18,
                    paired,
                    svc,
                } => (word, scratch, writes_x18, paired, svc),
            };
        let slot = self
            .slots
            .address(&[word, crate::platform_register::RETURN])?;
        let saved = scratch.map(|(index, value)| {
            let low = std::mem::replace(&mut registers.x[index], value);
            let high = paired.then(|| {
                let value = registers.x[19];
                std::mem::replace(&mut registers.x[index + 1], value)
            });
            (index, low, high)
        });
        registers.pc = slot;
        let exception = self.enter(epoch, *registers, tls, mappings);
        *registers = exception.registers;
        let completed = registers.pc == slot + 4;
        if let Some((index, original, saved_high)) = saved {
            if let Some(original_high) = saved_high {
                if completed {
                    registers.x[19] = registers.x[index + 1];
                }
                registers.x[index + 1] = original_high;
            }
            if completed && writes_x18 {
                registers.x[18] = registers.x[index];
            }
            registers.x[index] = original;
        }
        registers.pc = if completed { pc + 4 } else { pc };
        if unsafe { (*self.control).interrupted } {
            return stop(*registers, *tls, 0, 0, true);
        }
        if completed && exception.esr >> 26 == 0x3c && exception.esr & 0xffff == 0x5800 {
            return Ok(None);
        }
        if exception.registers.pc == slot
            && let Some(immediate) = svc
            && let Some(syscall) = exception.translated_syscall(pc, immediate)
        {
            registers.pc = syscall.elr;
            return stop(*registers, *tls, syscall.esr, 0, false);
        }
        stop(*registers, *tls, exception.esr.into(), exception.far, false)
    }

    pub fn run(
        &mut self,
        epoch: u64,
        mut registers: Registers,
        mut tls: u64,
        mappings: &BTreeMap<usize, Mapping>,
    ) -> io::Result<GuestStop> {
        let fetch = |pc: u64| {
            let pc = pc as usize;
            mappings.range(..=pc).next_back()?.1.fetch_word(pc)
        };
        unsafe { (*self.control).crossings.begin(mappings) };
        self.exclusive_initial = None;
        // After a detached STXR faults, the checked backend clears its
        // reservation. Follow the retry edge to capture the next load's
        // pre-state even when it resides in otherwise directly native text.
        let mut follow = if self.retry.take() == Some(registers.pc) {
            crate::exclusive::MAX_WORDS
        } else {
            0
        };
        loop {
            let pc = registers.pc as usize;
            let owner = mappings.range(..=pc).next_back().map(|(_, m)| m);
            let mut word = owner.and_then(|m| m.fetch_word(pc));
            if follow != 0 {
                let published = owner.and_then(|m| m.published_word(pc));
                if let Some(w) = published.filter(|w| crate::exclusive::retry_instruction(*w)) {
                    word = Some(w);
                    follow = if crate::exclusive::Access::decode(w).is_some() {
                        0
                    } else {
                        follow - 1
                    };
                } else {
                    follow = 0;
                }
            }
            let shadow = if word.is_some() && follow == 0 {
                match owner {
                    Some(m) => {
                        let linked = unsafe { (*self.control).crossings.entry(pc as u64, registers.x[30]) };
                        match linked {
                            Some(entry) => Some(entry),
                            None => m.shadow_entry(pc)?,
                        }
                    }
                    None => None,
                }
            } else {
                None
            };
            if let Some(entry) = shadow {
                registers.pc = entry;
            } else if let Some(word) = word {
                if let Some(stop) =
                    self.fetched_step(epoch, &mut registers, &mut tls, mappings, word)?
                {
                    return Ok(stop);
                }
                continue;
            }
            // The signal handler obtains the control block from its alternate
            // stack address. No borrow of the block spans the guest transition.
            let mut exception = self.enter(epoch, registers, &mut tls, mappings);
            let stopped_pc = exception.registers.pc;
            // Native returns may already have crossed back into ordinary guest
            // text. For an alias stop, recover the exact original instruction
            // before forwarding preemption, a data fault or a deliberate exit.
            let canonical = mappings
                .range(..=(stopped_pc as usize))
                .next_back()
                .is_some_and(|(_, m)| m.contains(stopped_pc as usize));
            let original = if canonical {
                None
            } else {
                unsafe { (*self.control).crossings.original_pc(stopped_pc) }
                    .or_else(|| owner.and_then(|m| m.original_shadow_pc(stopped_pc)))
                    .or_else(|| mappings.values().find_map(|m| m.original_shadow_pc(stopped_pc)))
            };
            if let Some(pc) = original {
                exception.registers.pc = pc;
            }
            registers = exception.registers;
            if unsafe { (*self.control).interrupted } {
                return Ok(GuestStop {
                    registers,
                    tls,
                    esr: 0,
                    far: 0,
                    interrupted: true,
                });
            }
            if original.is_some()
                && exception.esr >> 26 == 0x3c
                && exception.esr & 0xffff == fetched_instruction::shadow::EXIT_IMMEDIATE
            {
                continue;
            }
            // Signal-side discovery leaves code allocation to the worker.
            // The guest has not executed the patched instruction yet.
            if exception.esr >> 26 == 0x3c && exception.esr & 0xffff == 0x5700
                && unsafe { (*self.control).crossings.requested_native_link() }
            {
                unsafe { (*self.control).crossings.prepare(registers.pc, registers.x[30], registers.pc, mappings)? };
                continue;
            }
            // A native branch into an on-fetch region faults at its canonical
            // NX address. Only an instruction abort can start translation.
            if matches!(exception.esr >> 26, 0x20 | 0x21) && fetch(registers.pc).is_some() {
                unsafe {
                    (*self.control).crossings.prepare(registers.pc, registers.x[30], pc as u64, mappings)?;
                }
                continue;
            }
            // A native ELF scalar exclusive using x18 was patched before
            // entry. Run it with the same precise register renaming as a
            // fetched window, and follow a detached store's retry edge.
            if exception.esr == 1 << 25 {
                let pc = registers.pc as usize;
                if let Some((_, mapping)) = mappings.range(..=pc).next_back()
                    && let Some(patch) = mapping.instruction_at(pc)
                    && matches!(
                        mapping.patched_instruction(pc, patch),
                        Some(PatchedInstruction::PlatformRegister(
                            crate::platform_register::Action::Unsupported
                        ))
                    )
                    && let Some(word) = mapping.published_word(pc)
                    && let Some(access) = crate::exclusive::Access::decode(word)
                {
                    if let Some(stop) =
                        self.fetched_step(epoch, &mut registers, &mut tls, mappings, word)?
                    {
                        return Ok(stop);
                    }
                    if !access.load {
                        follow = crate::exclusive::MAX_WORDS;
                    }
                    continue;
                }
            }
            if exception.esr >> 26 == 0x3c {
                let pc = registers.pc as usize;
                if let Some((_, mapping)) = mappings.range(..=pc).next_back()
                    && let Some(word) = mapping.instruction_at(pc)
                {
                    if let Some(syscall) = exception.syscall(mapping, word) {
                        registers.pc = syscall.elr;
                        return Ok(GuestStop {
                            registers,
                            interrupted: false,
                            esr: syscall.esr,
                            far: 0,
                            tls,
                        });
                    }
                    match mapping.patched_instruction(pc, word) {
                        Some(PatchedInstruction::ReadThreadPointer { register }) => {
                            if let Some(destination) = registers.x.get_mut(register as usize) {
                                *destination = tls;
                            }
                            registers.pc += 4;
                            continue;
                        }
                        Some(PatchedInstruction::WriteThreadPointer { register }) => {
                            tls = registers.x.get(register as usize).copied().unwrap_or(0);
                            registers.pc += 4;
                            continue;
                        }
                        _ => {}
                    }
                }
            }
            return Ok(GuestStop {
                registers,
                interrupted: false,
                esr: exception.esr.into(),
                far: exception.far,
                tls,
            });
        }
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        // A directed signal already pending must not run on the host stack
        // after its control block is freed. This execution pthread now exits.
        mask_interrupt(libc::SIG_BLOCK).expect("Cannot mask worker interrupt");
        unsafe {
            *(&(*self.control).interrupt).thread.lock().unwrap() = None;
            self.control.drop_in_place();
            libc::sigaltstack(&self.previous_stack, ptr::null_mut());
            libc::munmap(self.mapping, STACK_SIZE);
        }
    }
}

#[cfg(target_os = "linux")]
use linux::signal_stack;

#[cfg(target_os = "macos")]
fn signal_stack() -> io::Result<*mut c_void> {
        // The signal trampoline masks SP to find Control at the bottom.
        // Request alignment from Mach to reserve exactly one stack.
        let rw = libc::VM_PROT_READ | libc::VM_PROT_WRITE;
        let mut address = 0;
        const VM_INHERIT_NONE: libc::vm_inherit_t = 2;
        #[allow(deprecated)]
        let status = unsafe {
            libc::mach_vm_map(
                libc::mach_task_self_, &mut address, STACK_SIZE as u64,
                STACK_SIZE as u64 - 1, libc::VM_FLAGS_ANYWHERE,
                0, 0, 0, rw, rw, VM_INHERIT_NONE,
            )
        };
        if status != libc::KERN_SUCCESS {
            return Err(io::Error::other(format!(
                "Cannot reserve worker signal stack: Mach error {status}"
            )));
        }
        Ok(ptr::with_exposed_provenance_mut::<c_void>(address as usize))
}

fn install_actions() -> io::Result<()> {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handle_signal as *const () as usize;
    action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
    unsafe {
        libc::sigfillset(&mut action.sa_mask);
        for signal in [
            libc::SIGUSR1,
            libc::SIGTRAP,
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
        ] {
            if libc::sigaction(signal, &action, ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn handle_signal(signal: i32, _: *mut libc::siginfo_t, raw: *mut c_void) {
    let stack: usize;
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) stack, options(nomem, nostack, preserves_flags))
    };
    let stack_base = stack & !(STACK_SIZE - 1);
    unsafe {
        // SA_ONSTACK supplies the owning thread's aligned stack. This worker
        // installs the handlers before entering guest code on that thread.
        let control = ptr::with_exposed_provenance_mut::<Control>(stack_base);
        let context = &mut *(*raw.cast::<libc::ucontext_t>()).uc_mcontext;
        let esr = context.__es.__esr;
        let breakpoint = signal == libc::SIGTRAP && esr >> 26 == 0x3c;
        let cancelled = (*control).epoch != 0
            && (&(*control).interrupt).epoch.load(Ordering::SeqCst) >= (*control).epoch;
        if signal == libc::SIGUSR1 && (!cancelled || (*control).host.is_none()) {
            return;
        }
        if (*control).host.is_some() {
            let control = &mut *control;
            let state = &mut context.__ss;
            let mut stop_esr = esr;
            let mut interrupted = signal == libc::SIGUSR1;
            if let Some(lr) = control.crossings.architectural_lr(state.__pc) {
                control.x18 = state.__lr;
                state.__lr = lr;
            } else if let Some(step) = control.step.take() {
                let completed = step.finish(state, &mut control.x18);
                if completed && breakpoint && esr & 0xffff == 0x5800 {
                    if !cancelled {
                        return;
                    }
                    interrupted = true;
                }
            } else if matches!(signal, libc::SIGBUS | libc::SIGSEGV)
                && matches!(esr >> 26, 0x20 | 0x21)
                && !cancelled
            {
                // A native branch can land on the canonical NX view. Resume
                // at an already published instruction without a worker round trip.
                // First publication stays outside the signal handler.
                let pc = state.__pc as usize;
                let mappings = &*control.mappings;
                if let Some(entry) = control.crossings.entry(pc as u64, state.__lr) {
                    state.__lr = control.x18;
                    state.__pc = entry;
                    return;
                }
                if let Some((_, mapping)) = mappings.range(..=pc).next_back()
                    && mapping.fetch_word(pc).is_some()
                    && !control.crossings.observe(pc as u64, state.__lr)
                    && let Some(entry) = mapping.cached_shadow_entry(pc)
                {
                    state.__pc = entry;
                    return;
                }
            } else if breakpoint && matches!(esr & 0xffff, 0x5400 | 0x5500) {
                // Execute a published TLS access in this signal return. Treat
                // ordinary guest BRKs with the same immediate as guest traps.
                let pc = state.__pc as usize;
                if let Some((_, mapping)) = (&*control.mappings).range(..=pc).next_back()
                    && let Some(word) = mapping.instruction_at(pc)
                    && let Some(instruction) = mapping.patched_instruction(pc, word)
                {
                    if cancelled {
                        interrupted = true;
                    } else {
                        match instruction {
                            PatchedInstruction::ReadThreadPointer { register } => {
                                control.write_register(state, register, control.tls);
                                state.__pc += 4;
                                return;
                            }
                            PatchedInstruction::WriteThreadPointer { register } => {
                                control.tls = control.read_register(state, register);
                                state.__pc += 4;
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            } else if breakpoint && esr & 0xffff == 0x5700 {
                // enter() borrows the registry for this guest transition.
                // All lookup data and RX words are immutable under its read lock.
                let pc = state.__pc as usize;
                let mappings = &*control.mappings;
                if let Some((_, mapping)) = mappings.range(..=pc).next_back()
                    && let Some(word) = mapping.instruction_at(pc)
                    && let Some(PatchedInstruction::PlatformRegister(action)) =
                        mapping.patched_instruction(pc, word)
                {
                    if cancelled {
                        interrupted = true;
                    } else if let Some(entry) = control.crossings.entry(pc as u64, state.__lr) {
                        state.__lr = control.x18;
                        state.__pc = entry;
                        return;
                    } else {
                        let learn = matches!(action, Action::NativeLoop { .. } | Action::Native { .. })
                            && control.crossings.observe_native(pc as u64, state.__lr, mapping, mappings);
                        if !learn {
                            if control.platform_trap(mapping, action, state) {
                                return;
                            }
                            stop_esr = 1 << 25;
                        }
                    }
                }
            }
            let mut exception = Exception::capture(context);
            exception.registers.x[18] = control.x18;
            exception.esr = stop_esr;
            control.interrupted = interrupted;
            control.stop = Some(exception);
            control.host.take().unwrap().restore(context);
        } else if breakpoint
            && esr & 0xffff == 0x5600
            && let Some(mut guest) = (*control).pending.take()
        {
            let mut host = Registers::capture(context);
            host.pc += 4;
            if cancelled {
                (*control).interrupted = true;
                (*control).stop = Some(Exception {
                    registers: guest,
                    esr: 0,
                    far: 0,
                });
                host.restore(context);
            } else {
                if (*control).crossings.architectural_lr(guest.pc).is_some() {
                    guest.x[30] = guest.x[18];
                }
                (*control).x18 = guest.x[18];
                guest.x[18] = host.x[18];
                (*control).host = Some(host);
                guest.restore(context);
            }
        } else {
            libc::_exit(128 + signal);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn x18_step_interrupt_commits_only_a_retired_instruction() {
        // A literal load seeds scratch with the literal address, which must
        // never become the guest's x18 when interrupted before the load.
        for completed in [false, true] {
            let step = NativeStep {
                guest_pc: 0x1000,
                step_pc: 0x2000,
                exit: 1,
                scratch: 0,
                saved: 0xcafe,
                saved_high: Some(0xdada),
            };
            let mut state: libc::__darwin_arm_thread_state64 = unsafe { std::mem::zeroed() };
            let mut x18 = 0xaaaa;
            state.__pc = if completed { 0x2004 } else { 0x2000 };
            state.__x[0] = if completed { 0xbbbb } else { 0x3000 };
            state.__x[1] = 0x1234;
            state.__x[19] = 0x5678;
            assert_eq!(step.finish(&mut state, &mut x18), completed);
            assert_eq!(state.__x[1], 0xdada);
            assert_eq!(state.__x[19], if completed { 0x1234 } else { 0x5678 });
            assert_eq!(state.__pc, if completed { 0x1004 } else { 0x1000 });
            assert_eq!(state.__x[0], 0xcafe);
            assert_eq!(x18, if completed { 0xbbbb } else { 0xaaaa });
        }
    }
}
