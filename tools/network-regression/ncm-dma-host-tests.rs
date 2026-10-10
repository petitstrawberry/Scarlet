//! Execute extracted encoder/parser with a counting allocator and guarded DMA slices.
#![allow(dead_code)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Counted;
static FREES_UNDER_QUEUE_LOCK: AtomicUsize = AtomicUsize::new(0);
static QUEUE_LOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if QUEUE_LOCKED.load(Ordering::Relaxed) {
            FREES_UNDER_QUEUE_LOCK.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static ALLOC: Counted = Counted;
/* PRODUCTION_NCM */
fn params(max: u16) -> CdcNcmParameters {
    CdcNcmParameters {
        formats_supported: 1,
        ntb_in_max_size: 16384,
        ndp_in_divisor: 4,
        ndp_in_payload_remainder: 0,
        ndp_in_alignment: 4,
        ntb_out_max_size: 16384,
        ndp_out_divisor: 4,
        ndp_out_payload_remainder: 0,
        ndp_out_alignment: 4,
        ntb_out_max_datagrams: max,
    }
}
fn config(max: u16) -> Ntb16TxConfig {
    Ntb16TxConfig::new(params(max), 1514, 1024).unwrap()
}
fn packet(len: usize, id: u8) -> DevicePacket {
    DevicePacket::with_data(vec![id; len])
}
#[test]
fn ten_mtu_frames_use_one_ntb_and_no_encoder_allocations() {
    let frames: Vec<_> = (0..10).map(|n| packet(1514, n)).collect();
    let mut dma = [0xa5; 16384];
    let before = ALLOCS.load(Ordering::Relaxed);
    let mut batch = Ntb16TxBatch::new(config(0));
    for p in frames {
        batch.push(p);
    }
    let len = batch.encode(&mut dma, 42).unwrap();
    let allocations = ALLOCS.load(Ordering::Relaxed) - before;
    assert_eq!(allocations, 0);
    assert_eq!(batch.count, 10);
    assert_eq!(batch.frame_bytes, 15140);
    assert!(!batch.accepts(&packet(1514, 11)));
    let decoded = parse_ntb16(&dma[..len], 1514).unwrap();
    assert_eq!(decoded.len(), 10);
    for (n, p) in decoded.iter().enumerate() {
        assert_eq!(p.data, vec![n as u8; 1514]);
    }
    assert!(dma[len..].iter().all(|x| *x == 0xa5));
    println!("10 Ethernet frames -> 1 NTB, encode allocations={allocations}, bytes={len}");
}
#[test]
fn respects_device_and_host_datagram_limits() {
    for limit in [0, 1, 2, 8, 16, 64] {
        let mut b = Ntb16TxBatch::new(config(limit));
        let count = if limit == 0 {
            16
        } else {
            usize::from(limit).min(16)
        };
        for _ in 0..count {
            assert!(b.accepts(&packet(60, 0)));
            b.push(packet(60, 0));
        }
        assert!(!b.accepts(&packet(60, 0)));
        let mut out = [0; 16384];
        let n = b.encode(&mut out, 0).unwrap();
        assert_eq!(parse_ntb16(&out[..n], 1514).unwrap().len(), count);
    }
}
#[test]
fn all_supported_alignment_remainders_roundtrip() {
    for divisor in [1, 2, 4, 8, 16, 32, 64] {
        for remainder in 0..divisor {
            let mut p = params(0);
            p.ndp_out_divisor = divisor;
            p.ndp_out_payload_remainder = remainder;
            p.ndp_out_alignment = divisor;
            let cfg = Ntb16TxConfig::new(p, 1514, 1024).unwrap();
            let mut b = Ntb16TxBatch::new(cfg);
            for n in [60, 61, 1514] {
                b.push(packet(n, n as u8));
            }
            let mut out = [0xa5; 16384];
            let n = b.encode(&mut out, 65535).unwrap();
            assert_eq!(read_u16(&out, 6).unwrap(), 65535);
            assert_eq!(b.ndp_offset % usize::from(divisor), 0);
            for i in 0..3 {
                assert_eq!(
                    (b.offsets[i] + 14) % usize::from(divisor),
                    usize::from(remainder)
                );
            }
            let d = parse_ntb16(&out[..n], 1514).unwrap();
            assert_eq!(d.iter().map(|p| p.len).collect::<Vec<_>>(), [60, 61, 1514]);
        }
    }
}
#[test]
fn max_packet_boundary_gets_initialized_short_packet_padding() {
    let cfg = config(1);
    let mut b = Ntb16TxBatch::new(cfg);
    let offset = align_to_remainder(b.end, cfg.payload_divisor, cfg.payload_remainder);
    b.push(packet(1024 - offset, 3));
    let mut out = [0xa5; 2048];
    assert_eq!(b.encode(&mut out, 1).unwrap(), 1025);
    assert_eq!(out[1024], 0);
    assert_eq!(out[1025], 0xa5);
}
#[test]
fn rejects_invalid_frame_or_undersized_dma_without_modifying_output() {
    let b = Ntb16TxBatch::new(config(0));
    assert!(!b.accepts(&packet(13, 0)));
    assert!(!b.accepts(&packet(1515, 0)));
    assert!(!b.accepts(&DevicePacket {
        data: vec![0; 14],
        len: 15
    }));
    let mut b = b;
    b.push(packet(60, 1));
    let mut out = [0xa5; 32];
    assert!(b.encode(&mut out, 0).is_err());
    assert_eq!(out, [0xa5; 32]);
    assert!(Ntb16TxBatch::new(config(0)).encode(&mut out, 0).is_err());
}
#[test]
fn fifo_head_stays_queued_when_ntb_size_is_full() {
    use std::collections::VecDeque;
    let mut queue: VecDeque<_> = (0..25).map(|n| packet(1514, n)).collect();
    let mut counts = Vec::new();
    let mut ids = Vec::new();
    while !queue.is_empty() {
        let mut b = Ntb16TxBatch::new(config(0));
        while queue.front().is_some_and(|p| b.accepts(p)) {
            b.push(queue.pop_front().unwrap());
        }
        counts.push(b.count);
        let mut out = [0; 16384];
        let len = b.encode(&mut out, 0).unwrap();
        ids.extend(
            parse_ntb16(&out[..len], 1514)
                .unwrap()
                .iter()
                .map(|p| p.data[0]),
        );
    }
    assert_eq!(counts, [10, 10, 5]);
    assert_eq!(ids, (0..25).collect::<Vec<_>>());
}
#[test]
fn rx_packets_survive_overwrite_of_original_dma() {
    let mut b = Ntb16TxBatch::new(config(0));
    b.push(packet(60, 7));
    b.push(packet(1514, 9));
    let mut dma = [0; 16384];
    let n = b.encode(&mut dma, 0).unwrap();
    let d = parse_ntb16(&dma[..n], 1514).unwrap();
    dma.fill(0xcc);
    assert_eq!(d[0].data, vec![7; 60]);
    assert_eq!(d[1].data, vec![9; 1514]);
}
#[test]
fn invalid_later_rx_pointer_rejects_entire_ntb() {
    let mut b = Ntb16TxBatch::new(config(0));
    b.push(packet(60, 7));
    b.push(packet(60, 9));
    let mut dma = [0; 16384];
    let n = b.encode(&mut dma, 0).unwrap();
    write_u16(&mut dma, b.ndp_offset + 12, 65530).unwrap();
    assert!(parse_ntb16(&dma[..n], 1514).is_err());
}
#[test]
fn minimum_ntb_and_one_frame_remain_supported() {
    let mut p = params(0);
    p.ntb_out_max_size = 2048;
    let mut b = Ntb16TxBatch::new(Ntb16TxConfig::new(p, 1514, 512).unwrap());
    b.push(packet(1514, 8));
    assert!(!b.accepts(&packet(1514, 9)));
    let mut out = [0; 2048];
    let n = b.encode(&mut out, 0).unwrap();
    assert_eq!(parse_ntb16(&out[..n], 1514).unwrap()[0].data, vec![8; 1514]);
}
#[test]
fn baseline_and_batched_frames_decode_identically() {
    for len in [14, 60, 512, 1024, 1514] {
        let p = packet(len, 42);
        let old = build_ntb16(p.as_slice(), 9, config(0)).unwrap();
        let mut b = Ntb16TxBatch::new(config(0));
        b.push(p);
        let mut out = [0; 16384];
        let n = b.encode(&mut out, 9).unwrap();
        assert_eq!(
            parse_ntb16(&old, 1514).unwrap()[0].data,
            parse_ntb16(&out[..n], 1514).unwrap()[0].data
        );
    }
}

mod rx_queue {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
    struct IrqSpinLock<T>(Mutex<T>);
    struct Guard<'a, T>(MutexGuard<'a, T>);
    impl<T> IrqSpinLock<T> {
        fn lock(&self) -> Guard<'_, T> {
            let g = self.0.lock().unwrap();
            assert!(!QUEUE_LOCKED.swap(true, Ordering::Relaxed));
            Guard(g)
        }
    }
    impl<T> std::ops::Deref for Guard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }
    impl<T> std::ops::DerefMut for Guard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.0
        }
    }
    impl<T> Drop for Guard<'_, T> {
        fn drop(&mut self) {
            QUEUE_LOCKED.store(false, Ordering::Relaxed);
        }
    }
    mod net_profile {
        pub struct Span;
        pub enum Stage {
            NcmQueueWait,
        }
        pub fn begin(_: Stage, _: usize, _: usize) -> Option<Span> {
            None
        }
    }
    fn ensure_rx_worker_started() {}
    struct Waker;
    impl Waker {
        fn wake_one(&self) {
            assert!(!QUEUE_LOCKED.load(Ordering::Relaxed));
        }
    }
    static RX_PACKET_WAKER: Waker = Waker;
    static RX_PACKET_QUEUE: LazyLock<IrqSpinLock<VecDeque<QueuedRxPacket>>> =
        LazyLock::new(|| IrqSpinLock(Mutex::new(VecDeque::with_capacity(NCM_RX_QUEUE_LIMIT))));
    /* PRODUCTION_RX_QUEUE */
    #[test]
    fn received_names_are_shared_and_queue_is_allocation_free() {
        drop(RX_PACKET_QUEUE.lock());
        let name: Arc<str> = Arc::from("usbnet1");
        let packets = (0..32).map(|i| packet(60, i)).collect();
        let before = ALLOCS.load(Ordering::Relaxed);
        assert_eq!(enqueue_rx_packets(&name, packets), 32);
        let after = ALLOCS.load(Ordering::Relaxed);
        assert_eq!(before, after);
        assert_eq!(Arc::strong_count(&name), 33);
        let mut next = 0;
        assert_eq!(
            drain_queued_rx_packets(&RX_PACKET_QUEUE, 64, |q| {
                assert!(!QUEUE_LOCKED.load(Ordering::Relaxed));
                assert!(Arc::ptr_eq(&q.interface_name, &name));
                assert_eq!(q.packet.data[0], next);
                next += 1;
            }),
            32
        );
        assert_eq!(Arc::strong_count(&name), 1);
    }
    #[test]
    fn queue_full_rejection_drops_remaining_packets_outside_irq_lock() {
        let name: Arc<str> = Arc::from("usbnet1");
        let packets = (0..NCM_RX_QUEUE_LIMIT + 10)
            .map(|_| packet(60, 1))
            .collect();
        let before = FREES_UNDER_QUEUE_LOCK.load(Ordering::Relaxed);
        assert_eq!(enqueue_rx_packets(&name, packets), NCM_RX_QUEUE_LIMIT);
        assert_eq!(FREES_UNDER_QUEUE_LOCK.load(Ordering::Relaxed), before);
        assert_eq!(
            drain_queued_rx_packets(&RX_PACKET_QUEUE, NCM_RX_QUEUE_LIMIT, |_| {}),
            NCM_RX_QUEUE_LIMIT
        );
    }
}
