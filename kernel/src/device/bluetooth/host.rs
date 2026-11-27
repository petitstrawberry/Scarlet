//! The controller worker drives Classic management and one HID session per ACL link.
//! Command and ACL transmit buffers belong to the controller.
use super::{
    H4Transport,
    classic::{Action, Address, Classic, ControllerInfo, Reason},
    control::{Request, Shared},
    h4,
    joycon::JoyCon,
    l2cap::{self, L2capSession},
};
use alloc::{sync::Arc, vec, vec::Vec};
use core::sync::atomic::Ordering;
use zeroize::Zeroizing;

const LINK_ERROR: u32 = 0x101;
const PROFILE_ERROR: u32 = 0x102;
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
    hid: Option<(L2capSession, JoyCon)>,
    deadline: Option<u64>,
    // Final management result while waiting for the addressed HCI disconnect.
    closing: Option<u32>,
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
}

impl Host {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
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
                    link.closing.get_or_insert(0);
                    link.deadline = None;
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
                    && let Some(i) = self.links.iter().position(|l| l.handle == hf & 0x0fff)
                    && self.links[i].closing.is_none()
                    && let Some((channels, _)) = &mut self.links[i].hid
                    && channels
                        .receive_acl(((hf >> 12) & 3) as u8, &frame[4..])
                        .is_err()
                {
                    self.fail_profile(i);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn identify_profile(&mut self, index: usize, now: u64) {
        let link = &mut self.links[index];
        if link.hid.is_some() || link.closing.is_some() {
            return;
        }
        let Some(name) = self.shared.state.lock().name(link.address) else {
            return;
        };
        let shared = self.shared.clone();
        let Some(profile) = JoyCon::connect(name.as_bytes(), move || {
            shared.queued.store(true, Ordering::Release);
            shared.wake.wake_one();
        }) else {
            if link.deadline.take().is_some() {
                self.shared.state.lock().profile_ready(link.address, false);
            }
            return;
        };
        let channels =
            L2capSession::new(link.handle, l2cap::DEFAULT_MTU).and_then(|mut channels| {
                channels.start()?;
                Ok(channels)
            });
        let Ok(channels) = channels else {
            return self.fail_profile(index);
        };
        link.hid = Some((channels, profile));
        link.deadline = Some(now.saturating_add(PROFILE_TIMEOUT_MS));
    }

    fn fail_profile(&mut self, index: usize) {
        let link = &mut self.links[index];
        link.hid = None;
        link.deadline = None;
        link.closing = Some(PROFILE_ERROR);
        self.shared
            .state
            .lock()
            .closing_after_failure(link.address, PROFILE_ERROR);
        if self.classic.disconnect_peer(link.address).is_err() {
            // Classic may already have queued the loss while delivering an earlier event.
            let address = link.address;
            self.links.remove(index);
            self.shared.state.lock().ended(address, PROFILE_ERROR);
        }
    }

    pub fn drive(&mut self, transport: &dyn H4Transport, now: u64) -> Result<(), &'static str> {
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
                    if let Some(i) = self.links.iter().position(|l| l.address == peer.address) {
                        self.identify_profile(i, now);
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
                    self.shared.state.lock().connected(address);
                    self.links.push(Link {
                        address,
                        handle,
                        hid: None,
                        closing: None,
                        deadline: Some(now.saturating_add(PROFILE_TIMEOUT_MS)),
                    });
                    self.identify_profile(self.links.len() - 1, now);
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
                        let error = match (link.closing, reason) {
                            (Some(0), Reason::LocalDisconnect) => 0,
                            (Some(PROFILE_ERROR), _) => PROFILE_ERROR,
                            _ => error_code(reason),
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
        for i in (0..self.links.len()).rev() {
            let link = &mut self.links[i];
            if link.closing.is_some() {
                continue;
            }
            let expired = link.deadline.is_some_and(|due| now >= due);
            if let Some((channels, joycon)) = &mut link.hid {
                if expired || !drive_hid(channels, joycon, now) {
                    self.fail_profile(i);
                } else if joycon.is_ready() && link.deadline.take().is_some() {
                    self.shared.state.lock().profile_ready(link.address, true);
                }
            } else if expired {
                // No remote name was resolved. Keep the generic encrypted link usable.
                link.deadline = None;
                self.shared.state.lock().profile_ready(link.address, false);
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
                if link.closing.is_none()
                    && let Some((channels, _)) = &mut link.hid
                    && let Some(fragment) = channels.poll_acl(usize::from(self.controller.acl_mtu))
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
        }
        Ok(())
    }

    pub fn next_deadline(&self) -> Option<u64> {
        let profile = self
            .links
            .iter()
            .filter(|link| link.closing.is_none())
            .flat_map(|link| {
                let hid = link
                    .hid
                    .as_ref()
                    .filter(|(c, _)| c.is_ready())
                    .and_then(|(_, joycon)| joycon.next_deadline_ms());
                [link.deadline, hid].into_iter().flatten()
            });
        self.classic
            .next_deadline_ms()
            .into_iter()
            .chain(profile)
            .min()
    }
}

fn drive_hid(channels: &mut L2capSession, joycon: &mut JoyCon, now: u64) -> bool {
    while let Some(event) = channels.poll_event() {
        match event {
            l2cap::Event::Closed(_) => return false,
            l2cap::Event::Interrupt(data) if data.first() == Some(&0xa1) => {
                if joycon.input_report(&data[1..]).is_err() {
                    return false;
                }
            }
            _ => {}
        }
    }
    if channels.is_ready() {
        match joycon.poll_output(now) {
            Ok(Some(report)) => {
                let mut packet = vec![0xa2]; // HIDP DATA, Output
                packet.extend_from_slice(&report);
                if channels.send_interrupt(&packet).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
            Ok(None) => {}
        }
    }
    true
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
