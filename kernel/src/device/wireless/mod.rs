//! Native wireless control sessions and a controlled Ethernet facade.
//! Explicit mocks cover EAPOL, key/port lifecycle and optional software CCMP.
//! Real radio/MPDU transport and hardware crypto offload require driver integration.

pub mod auth;
pub mod ccmp;
pub mod core;
pub mod data;
pub mod gate;
/// Shared wire types used by external wireless drivers.
pub use scarlet_abi::{network as network_abi, wireless as abi};
#[cfg(feature = "wireless-mock")]
pub mod mock;

extern crate alloc;
use self::core::{Radio, WirelessBackend};
use crate::network::NetworkInterface;
use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager},
    library::std::usercopy::{copy_from_user, copy_to_user},
    object::capability::{
        ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome},
    },
    sync::{IrqSpinLock, Lazy, Mutex, WaitResult, Waker},
};
use ::core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use alloc::{boxed::Box, string::String, sync::Arc, vec, vec::Vec};
use scarlet_abi::wireless::*;
use zeroize::{Zeroize, Zeroizing};

struct Shared {
    radio: Mutex<Radio>,
    waker: Waker,
    epoch: IrqSpinLock<u64>,
    port: Option<Arc<data::WirelessInterface>>,
}
impl Shared {
    fn synchronize(&self, radio: &mut Radio, now: u64) -> bool {
        let Some(port) = &self.port else {
            return true;
        };
        let result = port.reconcile(
            radio.device_epoch,
            radio.connection_id,
            radio.status.link_epoch,
            radio.status.phase == AUTHORIZED,
            radio.association(),
        );
        if result.is_err() {
            radio.complete_data_authorization(false, now);
            let _ = port.reconcile(
                radio.device_epoch,
                radio.connection_id,
                radio.status.link_epoch,
                false,
                radio.association(),
            );
        }
        radio.status.network_generation = crate::network::get_network_manager()
            .link_snapshot()
            .into_iter()
            .find(|link| link.id == radio.link_id)
            .map_or(0, |link| link.generation);
        if result.is_ok() {
            radio.complete_data_authorization(true, now);
        }
        result.is_ok()
    }
}

struct ControlDevice {
    shared: Arc<Shared>,
}
struct Session {
    shared: Arc<Shared>,
    id: u64,
    ready: Arc<AtomicBool>,
}
static NEXT_INDEX: AtomicUsize = AtomicUsize::new(0);
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);
static RADIOS: Lazy<IrqSpinLock<Vec<Arc<Shared>>>> = Lazy::new(|| IrqSpinLock::new(Vec::new()));

static ACTIVITY: Waker = Waker::new_uninterruptible("wireless-work");
static ACTIVITY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
struct Activity;
impl auth::WirelessNotifier for Activity {
    fn notify(&self) {
        notify_transport_change();
    }
}
/// Signal after publishing carrier, inbox or backend-completion changes.
/// IRQ-safe; does not acquire a radio or network lifecycle mutex.
pub fn notify_transport_change() {
    ACTIVITY_SEQUENCE.fetch_add(1, Ordering::Release);
    ACTIVITY.wake_one();
}
fn worker() {
    loop {
        let seen = ACTIVITY_SEQUENCE.load(Ordering::Acquire);
        let radios = RADIOS.lock().clone();
        let mut deadline = None;
        for shared in radios {
            let ready = {
                let mut radio = shared.radio.lock();
                let now = crate::timer::get_time_ns();
                radio.pump(now);
                shared.synchronize(&mut radio, now);
                *shared.epoch.lock() = radio.status.link_epoch;
                if let Some(next) = radio.next_deadline_ns() {
                    deadline = Some(deadline.map_or(next, |old: u64| old.min(next)));
                }
                radio.has_ready_sessions()
            };
            if ready {
                shared.waker.wake_all();
            }
        }
        if let Some(task) = crate::task::mytask() {
            let timeout = deadline.map(|end| end.saturating_sub(crate::timer::get_time_ns()));
            ACTIVITY.wait_with_condition(task.get_id(), task.get_trapframe(), timeout, 0, || {
                ACTIVITY_SEQUENCE.load(Ordering::Acquire) != seen
            });
        } else {
            return;
        }
    }
}

/// Publish a control endpoint for an already-registered Wi-Fi link. This does not
/// promise Ethernet transport or authorization: those are separate backend work.
pub fn register_control_device(
    link_id: u64,
    name: [u8; 32],
    backend: Box<dyn WirelessBackend>,
) -> Result<String, &'static str> {
    if backend.capabilities().features & FEATURE_DATA != 0 {
        return Err("wireless: data backend requires a controlled facade");
    }
    let manager = crate::network::get_network_manager();
    if !manager
        .link_snapshot()
        .iter()
        .any(|l| l.id == link_id && l.kind == scarlet_abi::network::LINK_KIND_WIFI)
    {
        return Err("wireless: no registered Wi-Fi link");
    }
    let shared = Arc::new(Shared {
        radio: Mutex::new(Radio::new(link_id, name, backend)),
        waker: Waker::new_interruptible("wireless"),
        epoch: IrqSpinLock::new(0),
        port: None,
    });
    publish(shared)
}
/// Register a wireless Ethernet facade over a transport that can retire its
/// unsubmitted queues. The transport's IRQ delivery must use the facade name
/// and stamp packets through NetworkInterface::prepare_receive before deferral.
pub fn register_data_device(
    name: &str,
    transport: Arc<dyn crate::network::NetworkInterface>,
    backend: Box<dyn WirelessBackend>,
) -> Result<String, &'static str> {
    if backend.capabilities().features & FEATURE_DATA == 0 {
        return Err("wireless: backend lacks data capability");
    }
    let manager = crate::network::get_network_manager();
    let port = Arc::new(data::WirelessInterface::new(
        String::from(name),
        transport,
        backend.capabilities().features & FEATURE_CCMP != 0,
    )?);
    manager.register_interface(name, port.clone())?;
    let id = manager
        .link_snapshot()
        .into_iter()
        .find(|l| l.interface_name() == Some(name))
        .ok_or("wireless: link registration missing")?
        .id;
    port.bind_link_id(id);
    if let Some(layer) = manager.get_layer("ethernet") {
        if let Some(ethernet) = layer
            .as_any()
            .downcast_ref::<crate::network::ethernet::EthernetLayer>()
        {
            ethernet.register_interface(name, port.mac_address(), port.clone());
        }
    }
    let mut raw_name = [0; 32];
    raw_name[..name.len()].copy_from_slice(name.as_bytes());
    let mut radio = Radio::new(id, raw_name, backend);
    radio.attach_authentication_port(port.clone());
    let shared = Arc::new(Shared {
        radio: Mutex::new(radio),
        waker: Waker::new_interruptible("wireless-data"),
        epoch: IrqSpinLock::new(0),
        port: Some(port),
    });
    match publish(shared) {
        Ok(control) => Ok(control),
        Err(error) => {
            manager.unregister_interface(name);
            Err(error)
        }
    }
}

fn publish(shared: Arc<Shared>) -> Result<String, &'static str> {
    let notifier = Arc::new(Activity);
    shared.radio.lock().attach_notifier(notifier.clone());
    if let Some(port) = &shared.port {
        port.attach_notifier(notifier);
    }
    let index = NEXT_INDEX
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1))
        .map_err(|_| "wireless: control indices exhausted")?;
    let name = alloc::format!("wifi{index}");
    DeviceManager::get_manager().register_device_with_name(
        name.clone(),
        Arc::new(ControlDevice {
            shared: Arc::clone(&shared),
        }),
    );
    RADIOS.lock().push(shared);
    notify_transport_change();
    if !WORKER_STARTED.swap(true, Ordering::AcqRel) {
        let task = crate::task::new_kernel_task(String::from("wireless"), 1, worker);
        task.init();
        crate::sched::scheduler::add_task(task, crate::arch::get_cpu().get_cpuid());
    }
    Ok(name)
}
impl Device for ControlDevice {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        let (id, ready) = {
            let mut radio = self.shared.radio.lock();
            let id = radio.open().map_err(|_| "wireless: cannot open session")?;
            (id, radio.readiness(id))
        };
        Ok(Arc::new(Session {
            shared: Arc::clone(&self.shared),
            id,
            ready,
        }))
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "wireless"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}
impl Device for Session {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "wireless-session"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let mut radio = self.shared.radio.lock();
        let now = crate::timer::get_time_ns();
        radio.close(self.id, now);
        self.shared.synchronize(&mut radio, now);
        *self.shared.epoch.lock() = radio.status.link_epoch;
        drop(radio);
        self.shared.waker.wake_all();
        notify_transport_change();
    }
}

macro_rules! no_stream_or_mapping {
    ($ty:ty) => {
        impl CharDevice for $ty {
            fn read_byte(&self) -> Option<u8> {
                None
            }
            fn write_byte(&self, _: u8) -> Result<(), &'static str> {
                Err("wireless: use control operations")
            }
            fn write(&self, _: &[u8]) -> Result<usize, &'static str> {
                Err("wireless: no stream writes")
            }
            fn can_read(&self) -> bool {
                false
            }
            fn can_write(&self) -> bool {
                false
            }
        }
        impl MemoryMappingOps for $ty {
            fn get_mapping_info(
                &self,
                _: usize,
                _: usize,
            ) -> Result<MemoryMappingInfo, &'static str> {
                Err("wireless: no mapping")
            }
            fn supports_mmap(&self) -> bool {
                false
            }
        }
    };
}
no_stream_or_mapping!(ControlDevice);
no_stream_or_mapping!(Session);
impl ControlOps for ControlDevice {}
impl Selectable for ControlDevice {
    fn current_ready(&self, _: ReadyInterest) -> ReadySet {
        ReadySet::none()
    }
    fn wait_until_ready(
        &self,
        _: ReadyInterest,
        _: &mut crate::arch::Trapframe,
        _: Option<u64>,
        _: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
}
impl ControlOps for Session {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        let task = crate::task::mytask().ok_or("wireless: no current task")?;
        let mut prefix = [0u8; HEADER_SIZE];
        copy_from_user(&task, arg, &mut prefix).map_err(|_| "wireless: invalid header pointer")?;
        let first = Header::decode(&prefix).ok_or("wireless: invalid header")?;
        let capacity = first.buffer_len as usize;
        if !(HEADER_SIZE..=MAX_BUFFER).contains(&capacity) || arg.checked_add(capacity).is_none() {
            return Err("wireless: invalid capacity");
        }
        let mut bytes = Zeroizing::new(vec![0; capacity]);
        copy_from_user(&task, arg, &mut bytes).map_err(|_| "wireless: invalid command buffer")?;
        let header = Header::decode(&bytes[..HEADER_SIZE]).unwrap();
        if header.buffer_len as usize != capacity {
            return Err("wireless: header changed during copy");
        }
        // Snapshot outside the radio mutex; NetworkManager may read link state.
        let link_id = self.shared.radio.lock().link_id;
        let generation = crate::network::get_network_manager()
            .link_snapshot()
            .into_iter()
            .find(|l| l.id == link_id)
            .map_or(0, |l| l.generation);
        let now = crate::timer::get_time_ns();
        // This is a sleepable task-context lock, not an IRQ spinlock. It also
        // serializes dup'd handles through copyout and event-cursor commit.
        let mut radio = self.shared.radio.lock();
        radio.status.network_generation = generation;
        radio.pump(now);
        self.shared.synchronize(&mut radio, now);
        let mut reply = radio.command(self.id, command, header, &bytes[HEADER_SIZE..], now);
        let synchronized = self.shared.synchronize(&mut radio, now);
        if !synchronized && (reply.header.result == ACCEPTED || command == SET_CONTROLLED_PORT) {
            reply.header.result = FAILED;
        }
        if command == MOCK_DATA_TEST && reply.header.result == OK {
            let request = DataTest::decode(&bytes[HEADER_SIZE..]).unwrap();
            match self
                .shared
                .port
                .as_ref()
                .ok_or("wireless: no data facade")?
                .test_control(request.operation)
            {
                Ok(stats) => {
                    reply.body = stats.bytes().to_vec();
                    reply.header.buffer_len = (HEADER_SIZE + reply.body.len()) as u32;
                }
                Err(_) => reply.header.result = FAILED,
            }
        }
        bytes.as_mut_slice().zeroize();
        bytes[..HEADER_SIZE].copy_from_slice(reply.header.bytes());
        if reply.body.len() <= capacity - HEADER_SIZE {
            bytes[HEADER_SIZE..HEADER_SIZE + reply.body.len()].copy_from_slice(&reply.body);
        }
        copy_to_user(&task, arg, &bytes)
            .map_err(|_| "wireless: response copy failed; query request ID")?;
        radio.commit(self.id, &reply);
        *self.shared.epoch.lock() = radio.status.link_epoch;
        drop(radio);
        self.shared.waker.wake_all();
        notify_transport_change();
        Ok(0)
    }
}
impl Selectable for Session {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let ready = self.ready.load(Ordering::Acquire);
        ReadySet {
            read: ready && interest.read,
            write: false,
            except: ready && interest.except,
        }
    }
    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        tf: &mut crate::arch::Trapframe,
        timeout: Option<u64>,
        min: u64,
    ) -> SelectWaitOutcome {
        if !interest.read && !interest.except {
            return SelectWaitOutcome::TimedOut;
        }
        let Some(task) = crate::task::mytask() else {
            return SelectWaitOutcome::TimedOut;
        };
        match self
            .shared
            .waker
            .wait_with_condition(task.get_id(), tf, timeout, min, || {
                self.ready.load(Ordering::Acquire)
            }) {
            WaitResult::Woken => SelectWaitOutcome::Ready,
            WaitResult::TimedOut => SelectWaitOutcome::TimedOut,
            WaitResult::Interrupted => SelectWaitOutcome::Interrupted,
        }
    }
}

#[cfg(feature = "wireless-mock")]
mod runtime {
    use super::*;
    use crate::{
        device::network::{DevicePacket, MacAddress},
        network::{InterfaceStats, NetworkInterface, ipv4::Ipv4Address},
        sync::IrqSpinLock,
    };
    struct MockLink {
        name: String,
        ip: IrqSpinLock<Option<Ipv4Address>>,
        shared: Arc<Shared>,
        mac: MacAddress,
    }
    impl NetworkInterface for MockLink {
        fn name(&self) -> &str {
            &self.name
        }
        fn mac_address(&self) -> MacAddress {
            self.mac
        }
        // Control-only mocks never advertise a usable data link, even if their
        // simulated association is AUTHORIZED. This cannot trigger DHCP.
        fn link_state(&self) -> u8 {
            scarlet_abi::network::LINK_DOWN
        }
        fn link_epoch(&self) -> u64 {
            *self.shared.epoch.lock()
        }
        fn link_kind(&self) -> u8 {
            scarlet_abi::network::LINK_KIND_WIFI
        }
        fn ip_address(&self) -> Option<Ipv4Address> {
            *self.ip.lock()
        }
        fn set_ip_address(&self, ip: Ipv4Address) {
            *self.ip.lock() = Some(ip);
        }
        fn clear_ip_address(&self) {
            *self.ip.lock() = None;
        }
        fn send(&self, _: DevicePacket) -> Result<(), &'static str> {
            Err("wireless mock: no packet transport")
        }
        fn poll(&self) -> Result<Vec<DevicePacket>, &'static str> {
            Ok(Vec::new())
        }
        fn stats(&self) -> InterfaceStats {
            InterfaceStats::default()
        }
    }
    pub fn init(cmdline: &str) -> Result<(), &'static str> {
        if !cmdline.split_whitespace().any(|s| s == "wifi.mock=1") {
            return Ok(());
        }
        let manager = crate::network::get_network_manager();
        let data_name = cmdline
            .split_whitespace()
            .find_map(|arg| arg.strip_prefix("wifi.mock.data="));
        for (index, mlme) in [MLME_FIRMWARE, MLME_HOST].into_iter().enumerate() {
            if index == 0 {
                if let Some(name) = data_name {
                    let transport = manager
                        .get_interface(name)
                        .ok_or("wireless mock: data transport missing")?;
                    if !transport.supports_packet_retirement() {
                        return Err("wireless mock: transport cannot retire packets");
                    }
                    manager.unregister_interface(name);
                    register_data_device(
                        name,
                        transport,
                        Box::new(
                            if cmdline
                                .split_whitespace()
                                .any(|arg| arg == "wifi.mock.ccmp=1")
                            {
                                mock::MockBackend::new(mlme, true).with_ccmp()
                            } else if cmdline
                                .split_whitespace()
                                .any(|arg| arg == "wifi.mock.wpa2=1")
                            {
                                mock::MockBackend::new(mlme, true).with_wpa2()
                            } else if cmdline
                                .split_whitespace()
                                .any(|arg| arg == "wifi.mock.auth=1")
                            {
                                mock::MockBackend::new(mlme, true).with_authentication()
                            } else {
                                mock::MockBackend::new(mlme, true).with_data()
                            },
                        ),
                    )?;
                    continue;
                }
            }
            let name = alloc::format!("wifimock{index}");
            let mut raw_name = [0; 32];
            raw_name[..name.len()].copy_from_slice(name.as_bytes());
            let shared = Arc::new(Shared {
                radio: Mutex::new(Radio::new(
                    0,
                    raw_name,
                    Box::new(mock::MockBackend::new(mlme, true)),
                )),
                waker: Waker::new_interruptible("wifi-mock"),
                epoch: IrqSpinLock::new(0),
                port: None,
            });
            let link = Arc::new(MockLink {
                name: name.clone(),
                ip: IrqSpinLock::new(None),
                shared: Arc::clone(&shared),
                mac: MacAddress::new([2, 0, 0, 0, 1, index as u8]),
            });
            manager.register_interface(&name, link)?;
            let id = manager
                .link_snapshot()
                .into_iter()
                .find(|l| l.interface_name() == Some(name.as_str()))
                .ok_or("mock link registration lost")?
                .id;
            shared.radio.lock().link_id = id;
            if let Err(e) = publish(Arc::clone(&shared)) {
                manager.unregister_interface(&name);
                return Err(e);
            }
        }
        Ok(())
    }
}
#[cfg(feature = "wireless-mock")]
pub use runtime::init as init_mocks;
