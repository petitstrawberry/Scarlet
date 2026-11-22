use scarlet::{
    hosted,
    interrupt::{InterruptClaim, InterruptManager, controllers::LocalInterruptType},
    timer::get_kernel_timer,
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

#[test]
fn hosted_interrupts_preserve_pending_delivery_and_cpu_ownership() {
    const IRQ: u32 = 12;
    static DELIVERIES: AtomicUsize = AtomicUsize::new(0);

    let _platform = hosted::prepare(4, 16 * 1024 * 1024).unwrap();
    let manager = InterruptManager::global();
    manager.init_controllers();
    manager.init_controllers_for_cpu(0);
    manager.init_controllers_for_cpu(1);
    manager
        .enable_local_interrupt(1, LocalInterruptType::Software)
        .unwrap();
    assert!(scarlet::arch::send_reschedule_ipi(1));
    assert!(manager.is_local_interrupt_pending(1, LocalInterruptType::Software));
    assert!(!manager.is_local_interrupt_pending(0, LocalInterruptType::Software));
    manager
        .disable_local_interrupt(1, LocalInterruptType::Software)
        .unwrap();
    manager.send_software_interrupt(0).unwrap();
    assert!(manager.is_local_interrupt_pending(0, LocalInterruptType::Software));
    manager.init_controllers_for_cpu(0);
    assert!(!manager.is_local_interrupt_pending(0, LocalInterruptType::Software));
    assert!(manager.is_local_interrupt_pending(1, LocalInterruptType::Software));
    manager
        .register_external_handler(IRQ, |_| {
            DELIVERIES.fetch_add(1, Ordering::Relaxed);
            Ok(InterruptClaim::Handled)
        })
        .unwrap();

    std::thread::spawn(|| hosted::raise_external_interrupt(0, IRQ).unwrap())
        .join()
        .unwrap();
    assert!(
        manager
            .claim_and_handle_pending_external_interrupt(0)
            .unwrap()
            .is_none()
    );
    assert!(
        manager
            .claim_and_handle_pending_external_interrupt(1)
            .unwrap()
            .is_none()
    );

    manager.enable_external_interrupt(IRQ, 0).unwrap();
    assert!(
        manager
            .claim_and_handle_pending_external_interrupt(0)
            .unwrap()
            .is_some()
    );
    assert!(
        manager
            .claim_and_handle_pending_external_interrupt(0)
            .unwrap()
            .is_none()
    );
    assert_eq!(DELIVERIES.load(Ordering::Relaxed), 1);

    let timer = get_kernel_timer();
    let now = timer.get_time_ns(0);
    timer.set_deadline_ns(0, now.saturating_add(5_000_000));
    timer.start(0);
    let limit = Instant::now() + Duration::from_secs(1);
    while !manager.is_local_interrupt_pending(0, LocalInterruptType::Timer) {
        assert!(Instant::now() < limit, "CPU 0 timer did not become pending");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!manager.is_local_interrupt_pending(1, LocalInterruptType::Timer));
    timer.stop(0);
    assert!(!manager.is_local_interrupt_pending(0, LocalInterruptType::Timer));
}
