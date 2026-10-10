//! Exact production claim/helper/TX/ring bodies, with host DMA/IRQ/cache boundaries.
//! RX preparation deliberately injects TX or teardown at unlocked boundaries.
#![allow(dead_code)]
extern crate alloc;
use environment::PAGE_SIZE;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

mod environment {
    pub const PAGE_SIZE: usize = 4096;
}
const COMMAND_COMPLETION_SUCCESS: u8 = 1;
const TRANSFER_EVENT_SHORT_PACKET: u8 = 13;
const USB_BULK_MAX_TRANSFER: usize = 16384;
const XHCI_CDC_NCM_TX_QUEUE_LIMIT: usize = 256;
const XHCI_CDC_NCM_TX_TRANSFER_DEPTH: usize = 8;
const XHCI_CDC_NCM_TX_WORK_BUDGET: usize = 8;
const XHCI_VERBOSE_TRACE: bool = false;
thread_local! {
    static HELD: Cell<bool> = const { Cell::new(false) };
    static CONTROLLER: Cell<*const XhciController> = const { Cell::new(std::ptr::null()) };
    static TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CURRENT_STAGE: Cell<Option<net_profile::Stage>> = const { Cell::new(None) };
    static ACTION: RefCell<Option<(&'static str, Action)>> = const { RefCell::new(None) };
    static RX_ADDRESSES: RefCell<HashMap<usize, usize>> = RefCell::new(HashMap::new());
    static PREPARED: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
}
fn note(s: impl Into<String>) {
    TRACE.with(|t| t.borrow_mut().push(s.into()));
}
fn unlocked(label: &str) {
    assert!(!HELD.with(Cell::get), "{label} while registry held");
}
fn with_controller(f: impl FnOnce(&XhciController)) {
    let ptr = CONTROLLER.with(Cell::get);
    assert!(!ptr.is_null());
    // Tests install this pointer only for a synchronous call with live controller.
    unsafe { f(&*ptr) }
}
#[derive(Clone, Copy)]
enum Action {
    Ack,
    Remove,
    Replace,
    Disconnect,
    ChangeDci,
    ChangeSize,
    RingFailure,
}
fn hook(boundary: &'static str) {
    unlocked(boundary);
    note(boundary);
    let action = ACTION.with(|a| {
        let mut a = a.borrow_mut();
        if a.as_ref().is_some_and(|(at, _)| *at == boundary) {
            a.take().map(|(_, x)| x)
        } else {
            None
        }
    });
    if let Some(action) = action {
        with_controller(|c| match action {
            Action::Ack => {
                // Execute the actual TX enqueue method, including its queue wake.
                c.enqueue_cdc_ncm_tx(1, 0x02, DevicePacket::with_data(vec![0x5a; 64]))
                    .unwrap();
                note("ack-enqueued");
            }
            Action::Remove | Action::Replace => {
                let removed = c.slot_runtime.lock().remove(0);
                drop(removed);
                if matches!(action, Action::Replace) {
                    c.slot_runtime.lock().push(slot(1, 100));
                }
                note("runtime-removed");
            }
            Action::Disconnect => {
                c.slot_runtime.lock()[0]
                    .cdc_ncm
                    .as_ref()
                    .unwrap()
                    .device
                    .attached
                    .store(false, Ordering::Release);
                note("runtime-inactive");
            }
            Action::ChangeDci => {
                c.slot_runtime.lock()[0]
                    .cdc_ncm
                    .as_mut()
                    .unwrap()
                    .bulk_in
                    .dci = 7;
            }
            Action::ChangeSize => {
                c.slot_runtime.lock()[0]
                    .cdc_ncm
                    .as_mut()
                    .unwrap()
                    .rx_transfer_size = 8192;
            }
            Action::RingFailure => {
                *c.slot_runtime.lock()[0]
                    .cdc_ncm
                    .as_ref()
                    .unwrap()
                    .bulk_in
                    .ring
                    .producer_index
                    .lock() = 15;
            }
        });
    }
}
struct IrqSpinLock<T> {
    inner: Mutex<T>,
    registry: bool,
}
struct Guard<'a, T> {
    guard: Option<MutexGuard<'a, T>>,
    registry: bool,
}
impl<T> IrqSpinLock<T> {
    fn new(v: T) -> Self {
        Self {
            inner: Mutex::new(v),
            registry: false,
        }
    }
    fn registry(v: T) -> Self {
        Self {
            inner: Mutex::new(v),
            registry: true,
        }
    }
    fn lock(&self) -> Guard<'_, T> {
        let guard = self
            .inner
            .try_lock()
            .expect("unexpected contended lock in host fixture");
        if self.registry {
            assert!(!HELD.with(|h| h.replace(true)));
            note("registry-lock");
        }
        Guard {
            guard: Some(guard),
            registry: self.registry,
        }
    }
}
impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().unwrap()
    }
}
impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().unwrap()
    }
}
impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if self.registry {
            assert!(HELD.with(|h| h.replace(false)));
            note("registry-unlock");
        }
    }
}
struct ContiguousPages {
    storage: UnsafeCell<Box<[u128]>>,
    id: usize,
}
impl ContiguousPages {
    fn new_aligned(n: usize, _: usize) -> Option<Self> {
        Some(Self {
            storage: UnsafeCell::new(vec![0; n * PAGE_SIZE / 16].into_boxed_slice()),
            id: 0,
        })
    }
    fn rx(id: usize) -> Self {
        let mut pages = Self::new_aligned(4, PAGE_SIZE).unwrap();
        pages.id = id;
        // Exclusively owned initialization of a modeled DMA buffer.
        unsafe {
            std::ptr::write_bytes(
                (&mut *pages.storage.get()).as_mut_ptr() as *mut u8,
                id as u8,
                16384,
            );
        }
        let address = pages.raw_address();
        RX_ADDRESSES.with(|a| a.borrow_mut().insert(address, id));
        pages
    }
    fn raw_address(&self) -> usize {
        unsafe { (&*self.storage.get()).as_ptr() as usize }
    }
    fn as_vaddr(&self) -> usize {
        if self.id != 0 && CURRENT_STAGE.with(Cell::get) == Some(net_profile::Stage::XhciRxCopy) {
            hook("copy");
        }
        self.raw_address()
    }
    fn len(&self) -> usize {
        unsafe { (&*self.storage.get()).len() * 16 / PAGE_SIZE }
    }
    fn as_paddr(&self) -> u64 {
        self.raw_address() as u64
    }
}
impl Drop for ContiguousPages {
    fn drop(&mut self) {
        if self.id != 0 {
            note(format!("free:{}:held={}", self.id, HELD.with(Cell::get)));
        }
    }
}
struct DmaMapping {
    id: usize,
    address: u64,
}
struct DmaAddress(u64);
impl DmaAddress {
    fn as_u64(&self) -> u64 {
        self.0
    }
}
impl DmaMapping {
    fn dma_addr(&self) -> DmaAddress {
        DmaAddress(self.address)
    }
}
impl Drop for DmaMapping {
    fn drop(&mut self) {
        note(format!("unmap:{}:held={}", self.id, HELD.with(Cell::get)));
    }
}
mod arch {
    use super::*;
    pub fn clean_invalidate_dcache_to_poc_range(address: usize, len: usize) {
        assert_eq!(len, 16384);
        hook("prepare");
        let id = RX_ADDRESSES.with(|a| *a.borrow().get(&address).unwrap());
        PREPARED.with(|p| p.borrow_mut().insert(id));
        note(format!("prepared:{id}:{len}"));
    }
    pub fn invalidate_dcache_to_poc_range(_: usize, len: usize) {
        if CURRENT_STAGE.with(Cell::get) == Some(net_profile::Stage::XhciRxInvalidate) {
            assert_eq!(len, 16384);
            hook("invalidate");
        }
    }
    pub fn clean_dcache_to_poc_range(address: usize, len: usize) {
        if CURRENT_STAGE.with(Cell::get) == Some(net_profile::Stage::XhciTxClean) {
            unlocked("tx clean");
            note(format!("tx-clean:{len}"));
            hook("tx-clean");
        }
        if CURRENT_STAGE.with(Cell::get) == Some(net_profile::Stage::XhciRxRequeue) && len == 16 {
            assert!(
                HELD.with(Cell::get),
                "ring publication without registry serialization"
            );
            // This is the actual production ring's aligned volatile TRB memory.
            let trb = unsafe { std::ptr::read_volatile(address as *const Trb) };
            if trb.trb_type() == TrbType::Normal as u8 {
                let id = ((trb.parameter - 0x200000) / 16384) as usize;
                assert!(
                    PREPARED.with(|p| p.borrow().contains(&id)),
                    "publication before this buffer preparation"
                );
                note("ring-publish");
            }
        }
    }
    pub fn rmb() {}
    pub fn io_wmb() {}
}
mod net_profile {
    use super::*;
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Stage {
        RxNtb,
        XhciRxInvalidate,
        XhciRxCopy,
        XhciRxRequeue,
        TxQueueFull,
        TxQueued,
        NcmTxBuild,
        XhciTxClean,
    }
    pub struct Span(Stage);
    pub fn begin(stage: Stage, bytes: usize, capacity: usize) -> Span {
        assert!(CURRENT_STAGE.with(|s| s.replace(Some(stage))).is_none());
        note(format!("begin:{stage:?}:{bytes}:{capacity}"));
        Span(stage)
    }
    impl Drop for Span {
        fn drop(&mut self) {
            assert_eq!(CURRENT_STAGE.with(|s| s.replace(None)), Some(self.0));
            note(format!("end:{:?}", self.0));
        }
    }
    pub fn event(stage: Stage, bytes: usize, capacity: usize) {
        note(format!("event:{stage:?}:{bytes}:{capacity}"));
    }
}
struct CdcNcmDevice {
    tx_completed: Mutex<(usize, usize)>,
    tx_errors: Mutex<usize>,
    attached: AtomicBool,
    received: Mutex<Vec<Vec<u8>>>,
    errors: Mutex<Vec<&'static str>>,
}
impl CdcNcmDevice {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            tx_completed: Mutex::new((0, 0)),
            tx_errors: Mutex::new(0),
            attached: AtomicBool::new(true),
            received: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        })
    }
    fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Acquire)
    }
    fn handle_receive_error(&self, error: &'static str) {
        unlocked("error callback");
        self.errors.lock().unwrap().push(error);
        note("error");
    }
    fn parse_received_ntb(&self, bytes: &[u8]) -> Result<Vec<DevicePacket>, &'static str> {
        hook("copy");
        Ok(vec![DevicePacket::with_data(bytes.to_vec())])
    }
    fn handle_received_packets(&self, packets: Result<Vec<DevicePacket>, &'static str>) {
        unlocked("packet callback");
        for p in packets.unwrap() {
            self.received.lock().unwrap().push(p.data);
        }
        note("delivery");
    }
    fn tx_batch(&self) -> Ntb16TxBatch {
        Ntb16TxBatch::new(Ntb16TxConfig {
            max_size: 16384,
            payload_divisor: 4,
            payload_remainder: 2,
            ndp_alignment: 4,
            max_segment_size: 1514,
            output_max_packet_size: 1024,
            max_datagrams: 16,
        })
    }
    fn encode_tx_batch(&self, b: &Ntb16TxBatch, output: &mut [u8]) -> Result<usize, &'static str> {
        unlocked("encode");
        let result = b.encode(output, 0);
        hook("tx-encode");
        result
    }
    fn handle_transmit_error(&self, _: &'static str, count: usize) {
        unlocked("tx error");
        *self.tx_errors.lock().unwrap() += count;
    }
    fn handle_transmit_complete(&self, bytes: usize, count: usize) {
        unlocked("tx complete");
        let mut s = self.tx_completed.lock().unwrap();
        s.0 += bytes;
        s.1 += count;
    }
}
struct Endpoint {
    endpoint_address: u8,
    dci: u8,
    ring: DmaTrbRing,
}
struct CdcNcmRuntime {
    device: Arc<CdcNcmDevice>,
    bulk_in: Endpoint,
    bulk_out: Endpoint,
    rx_in_flight: VecDeque<InFlightCdcNcmRx>,
    rx_available: Vec<CdcNcmDmaBuffer>,
    rx_transfer_size: usize,
    tx_queue: VecDeque<DevicePacket>,
    tx_preparing: usize,
    tx_in_flight: VecDeque<InFlightCdcNcmTx>,
    tx_available: Vec<CdcNcmDmaBuffer>,
    tx_transfer_size: usize,
}
struct UsbDevice(u8);
impl UsbDevice {
    fn slot_id(&self) -> u8 {
        self.0
    }
}
struct SlotRuntime {
    usb_device: UsbDevice,
    cdc_ncm: Option<CdcNcmRuntime>,
}
struct XhciController {
    slot_runtime: IrqSpinLock<Vec<SlotRuntime>>,
    doorbells: Mutex<Vec<(u8, u8)>>,
    wakes: Mutex<usize>,
}
impl XhciController {
    fn ring_endpoint_doorbell(&self, slot: u8, dci: u8) {
        unlocked("doorbell");
        if dci == 4 {
            self.doorbells.lock().unwrap().push((slot, dci));
            note("tx-doorbell");
            return;
        }
        // At the observable handoff there must be exactly eight owned requests.
        let slots = self.slot_runtime.lock();
        let ncm = slots[0].cdc_ncm.as_ref().unwrap();
        assert_eq!(ncm.rx_in_flight.len() + ncm.rx_available.len(), 8);
        let newest = ncm.rx_in_flight.back().unwrap();
        let index = ((newest.trb_dma - ncm.bulk_in.ring.dma_address()) / 16) as usize;
        assert_eq!(
            ncm.bulk_in.ring.peek(index).unwrap().parameter,
            newest.buffer.mapping.address
        );
        drop(slots);
        self.doorbells.lock().unwrap().push((slot, dci));
        note("doorbell");
    }
    fn queue_interrupt_work(&self, _: bool) {
        unlocked("TX work wake");
        *self.wakes.lock().unwrap() += 1;
        note("tx-wake");
    }
}
/* PRODUCTION_PATHS */

fn slot(id: u8, offset: usize) -> SlotRuntime {
    let stage = CURRENT_STAGE.with(|s| s.replace(None));
    let ring = DmaTrbRing::new_linked(16).unwrap();
    let mut rx_in_flight = VecDeque::with_capacity(8);
    for n in 1..=8 {
        let identity = n + offset;
        let buffer = CdcNcmDmaBuffer {
            mapping: DmaMapping {
                id: identity,
                address: 0x200000 + identity as u64 * 16384,
            },
            pages: ContiguousPages::rx(identity),
        };
        let index = ring
            .enqueue(Trb::normal_transfer_in(buffer.mapping.address, 16384))
            .unwrap();
        rx_in_flight.push_back(InFlightCdcNcmRx {
            buffer,
            trb_dma: ring.dma_address() + (index * 16) as u64,
        });
    }
    let result = SlotRuntime {
        usb_device: UsbDevice(id),
        cdc_ncm: Some(CdcNcmRuntime {
            device: CdcNcmDevice::new(),
            bulk_in: Endpoint {
                endpoint_address: 0x81,
                dci: 3,
                ring,
            },
            bulk_out: Endpoint {
                endpoint_address: 0x02,
                dci: 4,
                ring: DmaTrbRing::new_linked(16).unwrap(),
            },
            rx_in_flight,
            rx_available: Vec::with_capacity(8),
            rx_transfer_size: 16384,
            tx_queue: VecDeque::with_capacity(256),
            tx_preparing: 0,
            tx_in_flight: VecDeque::with_capacity(8),
            tx_transfer_size: 16384,
            tx_available: (0..8)
                .map(|_| CdcNcmDmaBuffer {
                    mapping: DmaMapping {
                        id: 0,
                        address: 0x800000,
                    },
                    pages: ContiguousPages::new_aligned(4, PAGE_SIZE).unwrap(),
                })
                .collect(),
        }),
    };
    CURRENT_STAGE.with(|s| s.set(stage));
    result
}
fn setup() -> Box<XhciController> {
    CONTROLLER.with(|p| p.set(std::ptr::null()));
    ACTION.with(|a| *a.borrow_mut() = None);
    CURRENT_STAGE.with(|s| s.set(None));
    PREPARED.with(|p| p.borrow_mut().clear());
    RX_ADDRESSES.with(|a| a.borrow_mut().clear());
    assert!(!HELD.with(Cell::get));
    let c = Box::new(XhciController {
        slot_runtime: IrqSpinLock::registry(vec![slot(1, 0)]),
        doorbells: Mutex::new(Vec::new()),
        wakes: Mutex::new(0),
    });
    CONTROLLER.with(|p| p.set(&*c));
    TRACE.with(|t| t.borrow_mut().clear());
    c
}
fn event(c: &XhciController, length: usize, code: u8) -> Trb {
    let pointer = c.slot_runtime.lock()[0]
        .cdc_ncm
        .as_ref()
        .unwrap()
        .rx_in_flight
        .front()
        .unwrap()
        .trb_dma;
    Trb {
        parameter: pointer,
        status: ((code as u32) << 24) | (16384 - length) as u32,
        control: (1 << 24) | (3 << 16) | (32 << 10),
    }
}
fn device(c: &XhciController) -> Arc<CdcNcmDevice> {
    c.slot_runtime.lock()[0]
        .cdc_ncm
        .as_ref()
        .unwrap()
        .device
        .clone()
}
fn trace() -> Vec<String> {
    TRACE.with(|t| t.borrow().clone())
}
fn arm(boundary: &'static str, action: Action) {
    ACTION.with(|a| *a.borrow_mut() = Some((boundary, action)));
}
fn count(c: &XhciController) -> (usize, usize) {
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    (n.rx_in_flight.len(), n.rx_available.len())
}
#[test]
fn successful_full_completion_keeps_depth_payload_and_publication_order() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 16384, 1);
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (8, 0));
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![1; 16384]]);
    assert!(d.errors.lock().unwrap().is_empty());
    assert_eq!(c.doorbells.lock().unwrap().as_slice(), &[(1, 3)]);
    let t = trace();
    let find = |s: &str| t.iter().position(|v| v == s).unwrap();
    assert!(find("invalidate") < find("copy"));
    assert!(find("copy") < find("prepare"));
    assert!(find("prepared:1:16384") < find("ring-publish"));
    assert!(find("ring-publish") < find("doorbell"));
    assert!(find("doorbell") < find("delivery"));
}
#[test]
fn short_completion_preserves_residual_length_and_low_event_pointer_bits() {
    let c = setup();
    let d = device(&c);
    let mut e = event(&c, 153, 13);
    e.parameter |= 7;
    assert!(c.handle_transfer_event(e));
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![1; 153]]);
    assert_eq!(count(&c), (8, 0));
    assert!(trace().contains(&"event:RxNtb:153:16384".into()));
}
#[test]
fn actual_ack_enqueue_succeeds_inside_each_unlocked_preparation_boundary() {
    for boundary in ["invalidate", "copy", "prepare"] {
        let c = setup();
        let e = event(&c, 1514, 1);
        arm(boundary, Action::Ack);
        assert!(c.handle_transfer_event(e));
        assert_eq!(*c.wakes.lock().unwrap(), 1);
        let slots = c.slot_runtime.lock();
        let ncm = slots[0].cdc_ncm.as_ref().unwrap();
        assert_eq!(ncm.tx_queue.len(), 1);
        assert_eq!(ncm.tx_queue[0].data, vec![0x5a; 64]);
        assert_eq!(ncm.tx_queue[0].len, 64);
        drop(slots);
        assert_eq!(count(&c), (8, 0));
    }
}
#[test]
fn stale_pointer_wrong_endpoint_and_absent_slot_do_not_claim_owners() {
    let c = setup();
    let mut e = event(&c, 64, 1);
    e.parameter = 0xdead0000;
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (8, 0));
    e.control = (1 << 24) | (8 << 16) | (32 << 10);
    assert!(!c.handle_transfer_event(e));
    e.control = (2 << 24) | (3 << 16) | (32 << 10);
    assert!(!c.handle_transfer_event(e));
    assert!(!trace().iter().any(|v| {
        [
            "invalidate",
            "copy",
            "prepare",
            "ring-publish",
            "delivery",
            "doorbell",
        ]
        .contains(&v.as_str())
    }));
}
#[test]
fn failed_and_zero_completions_recycle_without_payload_read() {
    for (code, expected) in [
        (6, "xHCI CDC-NCM bulk IN transfer failed"),
        (1, "CDC-NCM bulk IN completed without data"),
    ] {
        let c = setup();
        let d = device(&c);
        let e = event(&c, 0, code);
        assert!(c.handle_transfer_event(e));
        assert_eq!(count(&c), (8, 0));
        assert_eq!(d.errors.lock().unwrap().as_slice(), &[expected]);
        assert!(d.received.lock().unwrap().is_empty());
        assert!(
            !trace()
                .iter()
                .any(|v| ["invalidate", "copy"].contains(&v.as_str()))
        );
    }
}
#[test]
fn oversized_residual_saturates_to_zero_without_reading_payload() {
    let c = setup();
    let d = device(&c);
    let mut e = event(&c, 0, 1);
    e.status = (1 << 24) | 20000;
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (8, 0));
    assert_eq!(
        d.errors.lock().unwrap().as_slice(),
        &["CDC-NCM bulk IN completed without data"]
    );
    assert!(!trace().contains(&"copy".into()));
}
#[test]
fn disappearance_retires_local_buffer_unmap_before_free_after_unlock() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 64, 1);
    arm("prepare", Action::Remove);
    assert!(c.handle_transfer_event(e));
    assert!(c.slot_runtime.lock().is_empty());
    assert!(c.doorbells.lock().unwrap().is_empty());
    let t = trace();
    let unm = t.iter().position(|v| v == "unmap:1:held=false").unwrap();
    let freed = t.iter().position(|v| v == "free:1:held=false").unwrap();
    assert!(unm < freed);
    assert_eq!(t.iter().filter(|v| *v == "unmap:1:held=false").count(), 1);
    assert!(!t.contains(&"ring-publish".into()));
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![1; 64]]);
}
#[test]
fn same_slot_dci_new_arc_never_receives_completed_old_buffer() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 64, 1);
    arm("prepare", Action::Replace);
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (8, 0));
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&d, &n.device));
    assert!(n.rx_in_flight.iter().all(|r| r.buffer.mapping.id >= 101));
    drop(slots);
    assert!(c.doorbells.lock().unwrap().is_empty());
    assert!(!trace().contains(&"ring-publish".into()));
    assert_eq!(
        trace()
            .iter()
            .filter(|v| *v == "unmap:1:held=false")
            .count(),
        1
    );
}
#[test]
fn inactive_same_generation_returns_owner_without_republication() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 64, 1);
    arm("prepare", Action::Disconnect);
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (7, 1));
    assert!(c.doorbells.lock().unwrap().is_empty());
    assert!(!trace().contains(&"ring-publish".into()));
    assert!(!trace().iter().any(|v| v.starts_with("unmap:1:")));
    // Existing copied NTB delivery is retained even when device became inactive.
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![1; 64]]);
}
#[test]
fn same_arc_dci_or_size_change_rejected_before_publication() {
    for action in [Action::ChangeDci, Action::ChangeSize] {
        let c = setup();
        let d = device(&c);
        let e = event(&c, 64, 1);
        arm("prepare", action);
        assert!(c.handle_transfer_event(e));
        assert_eq!(count(&c), (7, 0));
        assert!(c.doorbells.lock().unwrap().is_empty());
        assert!(!trace().contains(&"ring-publish".into()));
        assert_eq!(
            d.errors.lock().unwrap().as_slice(),
            &["CDC-NCM receive runtime changed while preparing request"]
        );
        assert_eq!(
            trace()
                .iter()
                .filter(|v| *v == "unmap:1:held=false")
                .count(),
            1
        );
    }
}
#[test]
fn production_ring_failure_returns_exactly_one_owner_and_error_after_unlock() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 64, 1);
    arm("prepare", Action::RingFailure);
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (7, 1));
    assert_eq!(d.errors.lock().unwrap().as_slice(), &["Ring full"]);
    assert!(c.doorbells.lock().unwrap().is_empty());
    assert!(!trace().contains(&"ring-publish".into()));
    assert!(!trace().iter().any(|v| v.starts_with("unmap:1:")));
}
#[test]
fn repeated_exact_completions_cross_production_ring_wrap_with_eight_owners() {
    let c = setup();
    let d = device(&c);
    for _ in 0..60 {
        let e = event(&c, 64, 1);
        assert!(c.handle_transfer_event(e));
        assert_eq!(count(&c), (8, 0));
    }
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert_eq!(n.bulk_in.ring.current_producer_index(), 8);
    assert_eq!(n.bulk_in.ring.cycle_state(), true);
    let ids: HashSet<_> = n.rx_in_flight.iter().map(|r| r.buffer.mapping.id).collect();
    assert_eq!(ids, (1..=8).collect());
    drop(slots);
    assert_eq!(d.received.lock().unwrap().len(), 60);
    assert_eq!(c.doorbells.lock().unwrap().len(), 60);
    assert!(!trace().iter().any(|v| v.starts_with("unmap:")));
}

#[test]
fn exact_nonfront_completion_preserves_other_request_owners() {
    let c = setup();
    let d = device(&c);
    let mut e = event(&c, 23, 13);
    e.parameter = c.slot_runtime.lock()[0]
        .cdc_ncm
        .as_ref()
        .unwrap()
        .rx_in_flight[4]
        .trb_dma;
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (8, 0));
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![5; 23]]);
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert_eq!(n.rx_in_flight.front().unwrap().buffer.mapping.id, 1);
    assert_eq!(n.rx_in_flight.back().unwrap().buffer.mapping.id, 5);
}
#[test]
fn already_inactive_generation_skips_cache_preparation_and_returns_buffer() {
    let c = setup();
    let d = device(&c);
    let e = event(&c, 64, 1);
    d.attached.store(false, Ordering::Release);
    assert!(c.handle_transfer_event(e));
    assert_eq!(count(&c), (7, 1));
    assert!(c.doorbells.lock().unwrap().is_empty());
    assert!(
        !trace()
            .iter()
            .any(|v| v == "prepare" || v == "ring-publish" || v.starts_with("begin:XhciRxRequeue"))
    );
    assert_eq!(d.received.lock().unwrap().as_slice(), &[vec![1; 64]]);
}

fn queue_frames(c: &XhciController, count: usize) {
    for i in 0..count {
        c.enqueue_cdc_ncm_tx(1, 0x02, DevicePacket::with_data(vec![i as u8; 1514]))
            .unwrap();
    }
}
fn tx_event(c: &XhciController, residual: usize, code: u8) -> Trb {
    let pointer = c.slot_runtime.lock()[0]
        .cdc_ncm
        .as_ref()
        .unwrap()
        .tx_in_flight
        .front()
        .unwrap()
        .trb_dma;
    Trb {
        parameter: pointer,
        status: ((code as u32) << 24) | residual as u32,
        control: (1 << 24) | (4 << 16) | (32 << 10),
    }
}
#[test]
fn tx_batches_fifo_frames_into_dma_and_cleans_only_published_prefix() {
    let c = setup();
    queue_frames(&c, 25);
    assert_eq!(c.process_pending_cdc_ncm_tx(), 3);
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert_eq!(n.tx_available.len(), 5);
    assert_eq!(n.tx_preparing, 0);
    assert!(n.tx_queue.is_empty());
    assert_eq!(
        n.tx_in_flight
            .iter()
            .map(|r| r.frame_count)
            .collect::<Vec<_>>(),
        [10, 10, 5]
    );
    let mut ids = Vec::new();
    for r in &n.tx_in_flight {
        let dma = unsafe {
            std::slice::from_raw_parts(r.buffer.pages.as_vaddr() as *const u8, r.transfer_len)
        };
        ids.extend(parse_ntb16(dma, 1514).unwrap().iter().map(|p| p.data[0]));
        assert!(trace().contains(&format!("tx-clean:{}", r.transfer_len)));
    }
    assert_eq!(ids, (0..25).collect::<Vec<_>>());
    drop(slots);
    assert_eq!(c.doorbells.lock().unwrap().as_slice(), &[(1, 4)]);
}
#[test]
fn single_tx_frame_is_submitted_immediately_without_waiting_for_a_batch() {
    let c = setup();
    queue_frames(&c, 1);
    assert_eq!(c.process_pending_cdc_ncm_tx(), 1);
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert_eq!(n.tx_in_flight[0].frame_count, 1);
    assert_eq!(n.tx_available.len(), 7);
}
#[test]
fn tx_completion_returns_dma_buffer_and_accounts_all_frames() {
    let c = setup();
    let d = device(&c);
    queue_frames(&c, 10);
    c.process_pending_cdc_ncm_tx();
    assert!(c.handle_transfer_event(tx_event(&c, 0, 1)));
    assert_eq!(*d.tx_completed.lock().unwrap(), (15140, 10));
    let slots = c.slot_runtime.lock();
    let n = slots[0].cdc_ncm.as_ref().unwrap();
    assert_eq!(n.tx_available.len(), 8);
    assert!(n.tx_in_flight.is_empty());
}
#[test]
fn tx_failure_and_short_completion_account_entire_batch() {
    for (code, residual) in [(6, 0), (1, 1)] {
        let c = setup();
        let d = device(&c);
        queue_frames(&c, 10);
        c.process_pending_cdc_ncm_tx();
        assert!(c.handle_transfer_event(tx_event(&c, residual, code)));
        assert_eq!(*d.tx_errors.lock().unwrap(), 10);
        assert_eq!(*d.tx_completed.lock().unwrap(), (0, 0));
    }
}
#[test]
fn tx_queue_and_pipeline_are_bounded_and_resume_on_completion() {
    let c = setup();
    queue_frames(&c, 256);
    assert!(
        c.enqueue_cdc_ncm_tx(1, 2, DevicePacket::with_data(vec![0; 60]))
            .is_err()
    );
    assert_eq!(c.process_pending_cdc_ncm_tx(), 8);
    assert_eq!(c.process_pending_cdc_ncm_tx(), 0);
    {
        let slots = c.slot_runtime.lock();
        let n = slots[0].cdc_ncm.as_ref().unwrap();
        assert_eq!(n.tx_in_flight.len(), 8);
        assert_eq!(n.tx_queue.len(), 176);
        assert_eq!(n.tx_preparing, 0);
    }
    assert!(c.handle_transfer_event(tx_event(&c, 0, 1)));
    assert_eq!(c.process_pending_cdc_ncm_tx(), 1);
}
#[test]
fn tx_runtime_replacement_during_encode_never_publishes_old_buffer() {
    for action in [Action::Replace, Action::Remove, Action::Disconnect] {
        let c = setup();
        let old = device(&c);
        queue_frames(&c, 5);
        arm("tx-encode", action);
        assert_eq!(c.process_pending_cdc_ncm_tx(), 1);
        assert_eq!(*old.tx_errors.lock().unwrap(), 5);
        assert!(c.doorbells.lock().unwrap().is_empty());
        let slots = c.slot_runtime.lock();
        if let Some(slot) = slots.first() {
            let n = slot.cdc_ncm.as_ref().unwrap();
            assert_eq!(n.tx_preparing, 0);
            assert!(n.tx_in_flight.is_empty());
            assert_eq!(n.tx_available.len(), 8);
        }
    }
}
#[test]
fn tx_wrong_endpoint_invalid_frame_and_inactive_device_are_rejected() {
    let c = setup();
    assert!(
        c.enqueue_cdc_ncm_tx(1, 3, DevicePacket::with_data(vec![0; 60]))
            .is_err()
    );
    assert!(
        c.enqueue_cdc_ncm_tx(1, 2, DevicePacket::with_data(vec![0; 13]))
            .is_err()
    );
    device(&c).attached.store(false, Ordering::Release);
    assert!(
        c.enqueue_cdc_ncm_tx(1, 2, DevicePacket::with_data(vec![0; 60]))
            .is_err()
    );
    assert_eq!(*c.wakes.lock().unwrap(), 0);
}
