//! The controller worker drives Classic management, SDP and HID over encrypted ACL links.
//! Command and ACL transmit buffers belong to the controller.
use super::{
    H4Transport,
    classic::{Action, Address, Classic, ControllerInfo, Name, Reason},
    control::{Request, Shared},
    h4,
    l2cap::{self, L2capSession},
};
use crate::device::hid::{self, HidDevice, HidDeviceInfo, HidIdentity, HidTransport, HidWake};
use alloc::{sync::Arc, vec, vec::Vec};
use core::sync::atomic::Ordering;
use zeroize::Zeroizing;

const LINK_ERROR: u32 = 0x101;
const PROFILE_TIMEOUT_MS: u64 = 12_000;

fn error_code(reason: Reason) -> u32 {
    match reason {
        Reason::Hci(code) | Reason::HardwareError(code) if code != 0 => code.into(),
        _ => LINK_ERROR,
    }
}

struct Link {
    address: Address,
    handle: u16,
    name: Option<Name>,
    profile: Option<Profile>,
    disconnecting: bool,
}

enum Stage {
    Discovering,
    Opening(l2cap::sdp::HidService),
    Active(HidDevice),
    Closing,
}

struct Profile {
    channels: L2capSession,
    stage: Stage,
    deadline: Option<u64>,
    pending_output: Option<Vec<u8>>,
}

impl Profile {
    fn discover(handle: u16, now: u64) -> Option<Self> {
        Some(Self {
            channels: L2capSession::discover(handle, l2cap::DEFAULT_MTU).ok()?,
            stage: Stage::Discovering,
            deadline: Some(now.saturating_add(PROFILE_TIMEOUT_MS)),
            pending_output: None,
        })
    }

    fn active(&self) -> bool {
        matches!(self.stage, Stage::Active(_))
    }

    fn close(&mut self, now: u64) {
        if matches!(self.stage, Stage::Closing) {
            return;
        }
        self.stage = Stage::Closing; // Drops the binding and its hidraw node.
        self.pending_output = None;
        self.deadline = Some(now.saturating_add(PROFILE_TIMEOUT_MS));
        let _ = self.channels.disconnect();
    }

    fn next_deadline(&self, can_transmit: bool) -> Option<u64> {
        match &self.stage {
            Stage::Active(device) if can_transmit && self.pending_output.is_none() => {
                device.next_deadline_ms()
            }
            _ => self.deadline,
        }
    }

    fn queue_output(&mut self, now: u64) -> Result<(), &'static str> {
        let packet = if let Some(packet) = self.pending_output.take() {
            packet
        } else {
            let Stage::Active(device) = &mut self.stage else {
                return Ok(());
            };
            let Some(mut report) = device.poll_output(now)? else {
                return Ok(());
            };
            report.insert(0, 0xa2); // HIDP DATA, Output.
            report
        };
        match self.channels.send_interrupt(&packet) {
            Ok(()) => Ok(()),
            Err(l2cap::Error::QueueFull) => {
                self.pending_output = Some(packet);
                Ok(())
            }
            Err(_) => Err("cannot send HID output"),
        }
    }

    // Returns false after a terminal event. This ACL never reuses unconfirmed
    // channel IDs for a second profile, even if the peer stops answering.
    fn drive(
        &mut self,
        now: u64,
        handle: u16,
        transport: HidTransport,
        name: Option<Name>,
        wake: &HidWake,
        can_transmit: bool,
    ) -> bool {
        if self.deadline.is_some_and(|due| now >= due) {
            if matches!(self.stage, Stage::Closing) {
                return false;
            }
            self.close(now);
        }
        if self.active() && !self.channels.is_ready() {
            self.close(now);
        }
        while let Some(event) = self.channels.poll_event() {
            match event {
                l2cap::Event::Discovered(Some(service))
                    if matches!(self.stage, Stage::Discovering) =>
                {
                    // The SDP channel has closed. Only a valid HID record authorizes its PSMs.
                    if !hid::valid_report_descriptor(&service.report_descriptor) {
                        return false;
                    }
                    let Ok(mut channels) = L2capSession::for_hid(
                        handle,
                        l2cap::DEFAULT_MTU,
                        service.control_psm,
                        service.interrupt_psm,
                    ) else {
                        return false;
                    };
                    if channels.start().is_err() {
                        return false;
                    }
                    self.channels = channels;
                    self.stage = Stage::Opening(service);
                    self.deadline = Some(now.saturating_add(PROFILE_TIMEOUT_MS));
                }
                l2cap::Event::Discovered(_) | l2cap::Event::Closed(_) => return false,
                l2cap::Event::Ready => {
                    let Stage::Opening(service) = &mut self.stage else {
                        continue;
                    };
                    let Some(mtu) = self.channels.interrupt_tx_mtu() else {
                        self.close(now);
                        continue;
                    };
                    let info = HidDeviceInfo {
                        transport,
                        name: name
                            .as_ref()
                            .map_or_else(Vec::new, |n| n.as_bytes().to_vec()),
                        identity: service.device_id.as_ref().map(|id| HidIdentity {
                            vendor_id_source: id.vendor_id_source,
                            vendor_id: id.vendor_id,
                            product_id: id.product_id,
                            version: id.version,
                        }),
                        report_descriptor: core::mem::take(&mut service.report_descriptor),
                        max_output_len: usize::from(mtu.saturating_sub(1)),
                    };
                    match HidDevice::attach(info, wake.clone()) {
                        Ok(device) => {
                            self.stage = Stage::Active(device);
                            self.deadline = None;
                        }
                        Err(_) => self.close(now),
                    }
                }
                l2cap::Event::Interrupt(packet) => {
                    if let Stage::Active(device) = &mut self.stage
                        && let [0xa1, report @ ..] = packet.as_slice()
                        && device.input_report(report).is_err()
                    {
                        self.close(now);
                    }
                }
                l2cap::Event::Control(_) => {}
            }
        }
        if can_transmit && self.queue_output(now).is_err() {
            self.close(now);
        }
        true
    }
}

// A logical link may go down before the controller has flushed its ACL packets.
// Keep those debits until the matching completions or physical disconnect.
#[derive(Default)]
struct AclBuffers {
    capacity: u16,
    outstanding: Vec<(u16, u16)>,
}

impl AclBuffers {
    fn available(&self) -> u16 {
        self.capacity - self.outstanding.iter().map(|&(_, n)| n).sum::<u16>()
    }

    fn sent(&mut self, handle: u16) {
        if let Some((_, n)) = self.outstanding.iter_mut().find(|(h, _)| *h == handle) {
            *n += 1;
        } else {
            self.outstanding.push((handle, 1));
        }
    }

    fn completed(&mut self, handle: u16, count: u16) {
        if let Some(i) = self.outstanding.iter().position(|(h, _)| *h == handle) {
            let n = self.outstanding[i].1.saturating_sub(count);
            if n == 0 {
                self.outstanding.swap_remove(i);
            } else {
                self.outstanding[i].1 = n;
            }
        }
    }
}

pub(super) struct Host {
    shared: Arc<Shared>,
    classic: Classic,
    controller: ControllerInfo,
    links: Vec<Link>,
    buffers: AclBuffers,
    next_tx: usize,
    wake: HidWake,
    repoll: bool,
}

impl Host {
    pub fn new(shared: Arc<Shared>) -> Self {
        let notify = shared.clone();
        Self {
            wake: Arc::new(move || {
                notify.queued.store(true, Ordering::Release);
                notify.wake.wake_one();
            }),
            repoll: false,
            shared,
            classic: Classic::new(),
            controller: ControllerInfo::default(),
            links: Vec::new(),
            buffers: AclBuffers::default(),
            next_tx: 0,
        }
    }

    pub fn request(&mut self, op: Request) {
        let (addr, result) = match op {
            Request::Scan => {
                if self.classic.start_inquiry().is_err() {
                    self.shared.state.lock().finish_scan(LINK_ERROR);
                }
                return;
            }
            Request::Pair(addr) => (addr, self.classic.connect(addr)),
            Request::Connect(addr, key) => {
                if let Some(key) = key {
                    self.classic.provide_link_key(addr, *key);
                }
                (addr, self.classic.reconnect(addr))
            }
            Request::Disconnect(addr) => {
                let result = self.classic.disconnect_peer(addr);
                if result.is_ok()
                    && let Some(link) = self.links.iter_mut().find(|l| l.address == addr)
                {
                    link.disconnecting = true;
                    link.profile = None;
                    self.shared.state.lock().profile_ready(addr, false);
                }
                (addr, result)
            }
        };
        if result.is_err() {
            self.shared.state.lock().operation_failed(addr, LINK_ERROR);
        }
    }

    pub fn input(&mut self, now: u64, kind: u8, frame: &[u8]) -> Result<(), &'static str> {
        match kind {
            h4::EVENT => {
                self.classic
                    .on_event(now, frame[0], &frame[2..])
                    .map_err(|_| "invalid HCI event")?;
                // Disconnection Complete with success flushes all packets on this handle.
                if let [0x05, 4, 0, lo, hi, _] = frame {
                    self.buffers
                        .completed(u16::from_le_bytes([*lo, *hi]), u16::MAX);
                }
            }
            h4::ACL => {
                let hf = u16::from_le_bytes([frame[0], frame[1]]);
                if hf >> 14 == 0
                    && let Some(link) = self.links.iter_mut().find(|l| l.handle == hf & 0x0fff)
                    && let Some(profile) = &mut link.profile
                    && profile
                        .channels
                        .receive_acl(((hf >> 12) & 3) as u8, &frame[4..])
                        .is_err()
                {
                    profile.close(now);
                    self.shared.state.lock().profile_ready(link.address, false);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub fn drive(&mut self, transport: &dyn H4Transport, now: u64) -> Result<(), &'static str> {
        self.repoll = false;
        while let Some(action) = self.classic.poll(now) {
            match action {
                Action::Command(command) => {
                    let [lo, hi] = command.opcode.to_le_bytes();
                    let mut frame =
                        Zeroizing::new(vec![h4::COMMAND, lo, hi, command.params().len() as u8]);
                    frame.extend_from_slice(command.params());
                    transport.send(&frame)?;
                }
                Action::Ready(info) => {
                    self.controller = info;
                    self.buffers.capacity = info.acl_packets;
                    crate::println!(
                        "Bluetooth HCI ready: ACL MTU {} bytes, {} transmit buffers",
                        info.acl_mtu,
                        info.acl_packets
                    );
                    self.shared.state.lock().ready(info.address);
                }
                Action::PeerFound(peer) => {
                    self.shared.state.lock().update_peer(peer);
                    if let Some(name) = peer.name
                        && let Some(link) =
                            self.links.iter_mut().find(|l| l.address == peer.address)
                    {
                        link.name = Some(name);
                        if let Some(Profile {
                            stage: Stage::Active(device),
                            ..
                        }) = &link.profile
                        {
                            device.set_name(name.as_bytes());
                        }
                    }
                }
                Action::InquiryComplete(result) => self
                    .shared
                    .state
                    .lock()
                    .finish_scan(result.err().map_or(0, error_code)),
                Action::LinkKey {
                    address,
                    key,
                    key_type,
                } => {
                    self.shared.state.lock().set_key(address, key, key_type);
                }
                Action::LinkUp { address, handle } => {
                    let mut state = self.shared.state.lock();
                    state.connected(address);
                    let name = state.name(address);
                    drop(state);
                    self.links.push(Link {
                        address,
                        handle,
                        name,
                        profile: Profile::discover(handle, now),
                        disconnecting: false,
                    });
                }
                Action::LinkDown {
                    address,
                    handle,
                    reason,
                } => {
                    if let Some(i) = self
                        .links
                        .iter()
                        .position(|l| l.address == address && l.handle == handle)
                    {
                        let link = self.links.remove(i);
                        let error = if link.disconnecting && reason == Reason::LocalDisconnect {
                            0
                        } else {
                            error_code(reason)
                        };
                        drop(link);
                        self.shared.state.lock().ended(address, error);
                    }
                }
                Action::ConnectionFailed { address, reason } => {
                    self.shared.state.lock().ended(address, error_code(reason))
                }
                Action::AclCredits { handle, packets } => self.buffers.completed(handle, packets),
                Action::ControllerFault(_) => return Err("Bluetooth controller fault"),
            }
        }
        let can_transmit = self.buffers.available() != 0;
        for link in &mut self.links {
            let Some(profile) = &mut link.profile else {
                continue;
            };
            let was_ready = profile.active();
            let location = HidTransport::Bluetooth {
                controller: self.controller.address,
                peer: link.address,
            };
            let keep = profile.drive(
                now,
                link.handle,
                location,
                link.name,
                &self.wake,
                can_transmit,
            );
            let ready = keep && profile.active();
            if !keep {
                link.profile = None;
            }
            if ready != was_ready {
                self.shared.state.lock().profile_ready(link.address, ready);
            }
        }
        self.transmit(transport)
    }

    fn transmit(&mut self, transport: &dyn H4Transport) -> Result<(), &'static str> {
        // Keep the cursor across completion events, including one-buffer controllers.
        let count = self.links.len();
        while self.buffers.available() > 0 {
            let mut next = None;
            for _ in 0..count {
                let i = self.next_tx % count;
                self.next_tx = (i + 1) % count;
                let link = &mut self.links[i];
                if let Some(profile) = &mut link.profile
                    && let Some(fragment) = profile
                        .channels
                        .poll_acl(usize::from(self.controller.acl_mtu))
                {
                    next = Some(fragment);
                    break;
                }
            }
            let Some(fragment) = next else { break };
            let hf = fragment.handle | u16::from(fragment.pb) << 12;
            let mut frame = Vec::with_capacity(5 + fragment.data.len());
            frame.push(h4::ACL);
            frame.extend_from_slice(&hf.to_le_bytes());
            frame.extend_from_slice(&(fragment.data.len() as u16).to_le_bytes());
            frame.extend_from_slice(&fragment.data);
            transport.send(&frame)?;
            self.buffers.sent(fragment.handle);
            // Sending frees L2CAP queue space and can expose a final SDP/close event.
            self.repoll = true;
        }
        Ok(())
    }

    pub fn next_deadline(&self) -> Option<u64> {
        if self.repoll {
            return Some(0);
        }
        let can_transmit = self.buffers.available() != 0;
        let profile = self
            .links
            .iter()
            .filter_map(|link| link.profile.as_ref()?.next_deadline(can_transmit));
        self.classic
            .next_deadline_ms()
            .into_iter()
            .chain(profile)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(target_os = "none", test_case)]
    #[cfg_attr(not(target_os = "none"), test)]
    fn completions_and_disconnect_only_refund_their_own_acl_packets() {
        let mut pool = AclBuffers {
            capacity: 3,
            ..Default::default()
        };
        pool.sent(11);
        pool.sent(22);
        pool.sent(22);
        assert_eq!(pool.available(), 0);
        pool.completed(33, u16::MAX);
        assert_eq!(pool.available(), 0);
        pool.completed(22, 1);
        assert_eq!(pool.available(), 1);
        pool.completed(11, u16::MAX); // physical disconnect, including unreported completions
        pool.completed(11, 1); // already refunded
        assert_eq!(pool.available(), 2);
        pool.sent(11); // reused Bluetooth handle
        pool.completed(22, 9);
        assert_eq!(pool.available(), 2);
        pool.completed(11, 1);
        assert_eq!(pool.available(), 3);
    }
}
