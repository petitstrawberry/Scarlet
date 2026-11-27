//! Native Classic Bluetooth controllers. A board supplies H4 byte transport;
//! controller management, pairing, L2CAP and HID input run on a common worker.
extern crate alloc;
mod classic;
mod control;
mod h4;
mod host;
mod joycon;
mod l2cap;

use crate::{
    device::manager::DeviceManager,
    sync::{IrqSpinLock, Waker},
};
use alloc::{collections::VecDeque, format, sync::Arc};
use control::{Endpoint, Shared};
use core::sync::atomic::{AtomicUsize, Ordering};
use h4::Decoder;
use host::Host;

/// The worker exclusively owns the transport. start/stop may sleep. RX wakes
/// the supplied waker after publishing bytes. receive drains queued bytes
/// without waiting and send writes one complete H4 packet or reports failure.
pub trait H4Transport: Send + Sync {
    fn start(&self, wake: Arc<Waker>) -> Result<(), &'static str>;
    fn receive(&self, buffer: &mut [u8]) -> Result<usize, &'static str>;
    fn rx_pending(&self) -> bool;
    fn send(&self, frame: &[u8]) -> Result<(), &'static str>;
    fn stop(&self);
}

struct Controller {
    number: usize,
    shared: Arc<Shared>,
    transport: Arc<dyn H4Transport>,
}

static PENDING: IrqSpinLock<VecDeque<Controller>> = IrqSpinLock::new(VecDeque::new());

/// Publish an adapter and start its worker. Controller initialization runs off
/// the device-probe thread so a missing radio cannot hold up the boot path.
pub fn register(transport: Arc<dyn H4Transport>) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let shared = Arc::new(Shared::new());
    let name = format!("bluetooth{number}");
    DeviceManager::get_manager().register_device_with_name(
        name.clone(),
        Arc::new(Endpoint {
            shared: shared.clone(),
            id: 0,
        }),
    );
    PENDING.lock().push_back(Controller {
        number,
        shared,
        transport,
    });
    let task = crate::task::new_kernel_task(name, 1, worker);
    task.init();
    crate::sched::scheduler::add_task(task, crate::arch::get_cpu().get_cpuid());
}

fn worker() {
    let controller = PENDING.lock().pop_front();
    let Some(controller) = controller else { return };
    let mut last_error = None;
    loop {
        let result = controller
            .transport
            .start(controller.shared.wake.clone())
            .and_then(|()| serve(&controller));
        controller.transport.stop();
        controller.shared.state.lock().lost_controller();
        if let Err(error) = result
            && last_error != Some(error)
        {
            crate::println!("bluetooth{}: {}", controller.number, error);
            last_error = Some(error);
        }
        if let Some(task) = crate::task::mytask() {
            task.sleep(task.get_trapframe(), 5_000_000_000);
        }
    }
}

fn serve(controller: &Controller) -> Result<(), &'static str> {
    let transport = &*controller.transport;
    let shared = &controller.shared;
    let mut host = Host::new(shared.clone());
    let mut rx = Decoder::default();
    let mut buffer = [0u8; 512];
    let task = crate::task::mytask().ok_or("missing adapter task")?;
    loop {
        let now = crate::time::current_time_ns() / 1_000_000;
        let request = {
            let mut state = shared.state.lock();
            // Consume published work before checking requests and HID output mailboxes.
            shared.queued.swap(false, Ordering::Acquire);
            state.request.take()
        };
        if let Some(op) = request {
            host.request(op);
        }
        host.drive(transport, now)?;
        let count = transport.receive(&mut buffer)?;
        if count != 0 {
            let mut result = Ok(());
            rx.feed(
                buffer.get(..count).ok_or("invalid transport byte count")?,
                |kind, frame| {
                    if result.is_ok() {
                        result = host
                            .input(now, kind, frame)
                            .and_then(|()| host.drive(transport, now));
                    }
                },
            )?;
            result?;
            continue;
        }
        let now = crate::time::current_time_ns() / 1_000_000;
        let timeout = host
            .next_deadline()
            .map(|when| when.saturating_sub(now).saturating_mul(1_000_000));
        if timeout == Some(0) {
            continue;
        }
        shared
            .wake
            .wait_with_condition(task.get_id(), task.get_trapframe(), timeout, 0, || {
                transport.rx_pending() || shared.queued.load(Ordering::Acquire)
            });
    }
}
