//! Deferred Ethernet ingress shared by interrupt-driven drivers.

use crate::{
    device::network::DevicePacket,
    network::get_network_manager,
    sync::{IrqSpinLock, Lazy},
};
use alloc::{collections::VecDeque, string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, Ordering};

const RX_WORKER_QUEUE_LIMIT: usize = 1024;
const RX_WORKER_BUDGET: usize = 64;

struct QueuedRxPacket {
    interface_name: Arc<str>,
    packet: DevicePacket,
}

static RX_WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static RX_PACKET_QUEUE: Lazy<IrqSpinLock<VecDeque<QueuedRxPacket>>> =
    Lazy::new(|| IrqSpinLock::new(VecDeque::new()));
static RX_PACKET_WAKER: crate::sync::Waker = crate::sync::Waker::new_uninterruptible("net-rx");

/// Start the shared ingress Task before enabling a driver's receive IRQ.
pub fn ensure_rx_worker_started() {
    if RX_WORKER_STARTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    let mut task = crate::task::new_kernel_task(String::from("net-rx"), 1, rx_worker_entry);
    task.init();
    crate::sched::scheduler::add_task(task, crate::arch::get_cpu().get_cpuid());
}

fn rx_worker_entry() {
    loop {
        let processed = drain_queued_rx_packets(RX_WORKER_BUDGET);

        let Some(task) = crate::task::mytask() else {
            crate::arch::instruction::idle();
        };

        if processed == RX_WORKER_BUDGET {
            // The queue guard is released before this scheduling point. Bound
            // each pass so sustained ingress cannot starve the socket reader.
            crate::sched::scheduler::schedule(task.get_trapframe());
        } else {
            RX_PACKET_WAKER.wait(task.get_id(), task.get_trapframe());
        }
    }
}

fn drain_queued_rx_packets(budget: usize) -> usize {
    let manager = crate::network::get_network_manager();
    crate::network::tcp::begin_rx_batch();
    let mut processed = 0usize;
    while processed < budget {
        let Some(queued) = pop_queued_rx_packet() else {
            break;
        };
        manager.handle_received_packet(&queued.interface_name, &queued.packet);
        processed += 1;
        if processed.is_multiple_of(crate::network::tcp::TX_ACK_INPUT_BATCH_SIZE) {
            crate::network::tcp::flush_ack_input();
        }
        if processed.is_multiple_of(crate::network::tcp::RX_NOTIFY_BATCH_SIZE) {
            crate::network::tcp::flush_rx_batch();
        }
    }
    crate::network::tcp::end_rx_batch();
    processed
}

fn pop_queued_rx_packet() -> Option<QueuedRxPacket> {
    RX_PACKET_QUEUE.lock().pop_front()
}

/// Prepare received packets and enqueue them for bounded Task-context delivery.
pub fn enqueue_rx_packets(interface_name: Arc<str>, mut packets: Vec<DevicePacket>) -> usize {
    if packets.is_empty() {
        return 0;
    }

    ensure_rx_worker_started();

    // Stamp the packet before deferring delivery, preserving the link epoch
    // even when a controlled interface changes state before the worker runs.
    let Some(interface) = get_network_manager().get_interface(&interface_name) else {
        return 0;
    };
    packets.retain_mut(|packet| interface.prepare_receive(packet));

    let mut enqueued = 0usize;
    {
        let mut queue = RX_PACKET_QUEUE.lock();
        for packet in packets {
            if queue.len() >= RX_WORKER_QUEUE_LIMIT {
                interface.receive_dropped(&packet);
                continue;
            }

            queue.push_back(QueuedRxPacket {
                interface_name: interface_name.clone(),
                packet,
            });
            enqueued += 1;
        }
    }

    if enqueued > 0 {
        RX_PACKET_WAKER.wake_one();
    }

    enqueued
}
