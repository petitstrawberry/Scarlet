use crate::arch::Trapframe;
use crate::sched::scheduler::get_scheduler;
use crate::interrupt::manager::InterruptManager;

pub fn arch_interrupt_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        5 => {
            // Timer interrupt - use new interrupt management system
            if let Ok(manager_lock) = InterruptManager::get() {
                let mut manager = manager_lock.write();
                if let Some(ref mut manager) = *manager {
                    match manager.handle_interrupt(crate::interrupt::irq::TIMER_INTERRUPT, trapframe) {
                        Ok(()) => {
                            // Timer interrupt handled successfully by new system
                        }
                        Err(_) => {
                            // Fallback to old timer handling
                            let scheduler = get_scheduler();
                            scheduler.schedule(trapframe);
                        }
                    }
                } else {
                    // Fallback to old timer handling if manager not initialized
                    let scheduler = get_scheduler();
                    scheduler.schedule(trapframe);
                }
            } else {
                // Fallback to old timer handling
                let scheduler = get_scheduler();
                scheduler.schedule(trapframe);
            }
        }
        irq => {
            // Handle other interrupts through the new system
            if let Ok(manager_lock) = InterruptManager::get() {
                let mut manager = manager_lock.write();
                if let Some(ref mut manager) = *manager {
                    match manager.handle_interrupt(irq, trapframe) {
                        Ok(()) => {
                            // Interrupt handled successfully
                        }
                        Err(msg) => {
                            crate::println!("[Interrupt] Failed to handle IRQ {}: {}", irq, msg);
                            // For now, just loop for unknown interrupts
                            loop {}
                        }
                    }
                } else {
                    crate::println!("[Interrupt] Manager not initialized, unknown IRQ: {}", irq);
                    loop {}
                }
            } else {
                crate::println!("[Interrupt] Cannot access interrupt manager, unknown IRQ: {}", irq);
                loop {}
            }
        }
    }
}