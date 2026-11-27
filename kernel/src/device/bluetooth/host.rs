//! A single controller owns command credits, ACL credits and the negotiated HID
//! session. All radio state transitions run on its worker, outside interrupt context.
use super::{
    H4Transport,
    classic::{self, Action, Classic, ControllerInfo, Reason},
    control::{Request, Shared},
    h4,
    joycon::JoyCon,
    l2cap::{self, L2capSession},
};
use alloc::{sync::Arc, vec, vec::Vec};
use scarlet_abi::bluetooth as abi;
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
    handle: u16,
    channels: Option<L2capSession>,
    joycon: Option<JoyCon>,
    deadline: Option<u64>,
}

pub(super) struct Host {
    shared: Arc<Shared>,
    classic: Classic,
    controller: ControllerInfo,
    link: Option<Link>,
    credits: u16,
    disconnecting: bool,
    profile_failed: bool,
    repoll: bool,
}

impl Host {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            classic: Classic::new(),
            controller: ControllerInfo::default(),
            link: None,
            credits: 0,
            disconnecting: false,
            profile_failed: false,
            repoll: false,
        }
    }

    pub fn request(&mut self, op: Request) {
        let result = match op {
            Request::Scan => self.classic.start_inquiry(),
            Request::Pair(addr) => self.classic.connect(addr),
            Request::Connect(addr, key) => {
                if let Some(key) = key {
                    self.classic.provide_link_key(addr, *key);
                }
                self.classic.reconnect(addr)
            }
            Request::Disconnect => {
                self.disconnecting = true;
                self.classic.disconnect()
            }
        };
        if result.is_err() {
            self.disconnecting = false;
            let mut state = self.shared.state.lock();
            if self.link.is_none() {
                state.peer = [0; 6];
            }
            state.finish(LINK_ERROR);
        }
    }

    pub fn input(&mut self, now: u64, kind: u8, frame: &[u8]) -> Result<(), &'static str> {
        match kind {
            h4::EVENT => self
                .classic
                .on_event(now, frame[0], &frame[2..])
                .map_err(|_| "invalid HCI event")?,
            h4::ACL => {
                let hf = u16::from_le_bytes([frame[0], frame[1]]);
                if let Some(link) = self.link.as_mut()
                    && hf & 0x0fff == link.handle
                    && hf >> 14 == 0
                    && let Some(channels) = &mut link.channels
                    && channels
                        .receive_acl(((hf >> 12) & 3) as u8, &frame[4..])
                        .is_err()
                {
                    self.fail_profile();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn identify_profile(&mut self, now: u64) {
        let Some(link) = &mut self.link else { return };
        if link.channels.is_some() || self.disconnecting || self.profile_failed {
            return;
        }
        let mut state = self.shared.state.lock();
        let Some(name) = state
            .peers
            .iter()
            .find(|p| p.address == state.peer)
            .and_then(|p| p.name)
        else {
            return;
        };
        let Some(profile) = JoyCon::connect(name.as_bytes()) else {
            link.deadline = None;
            state.finish(0);
            return;
        };
        drop(state);
        let Ok(mut channels) = L2capSession::new(link.handle, l2cap::DEFAULT_MTU) else {
            self.fail_profile();
            return;
        };
        if channels.start().is_err() {
            self.fail_profile();
            return;
        }
        link.joycon = Some(profile);
        link.channels = Some(channels);
        link.deadline = Some(now.saturating_add(PROFILE_TIMEOUT_MS));
    }

    fn fail_profile(&mut self) {
        if let Some(link) = &mut self.link {
            link.channels = None;
            link.joycon = None;
            link.deadline = None;
        }
        self.profile_failed = true;
        self.repoll = true;
        let mut state = self.shared.state.lock();
        state.flags &= !abi::PROFILE_READY;
        state.flags |= abi::BUSY;
        drop(state);
        let _ = self.classic.disconnect();
    }

    fn link_ended(&mut self, reason: Reason) {
        self.link = None;
        self.credits = self.controller.acl_packets;
        let mut state = self.shared.state.lock();
        state.peer = [0; 6];
        state.flags &= !(abi::CONNECTED | abi::ENCRYPTED | abi::PROFILE_READY);
        state.clear_key();
        let code = if self.profile_failed {
            PROFILE_ERROR
        } else if self.disconnecting {
            0
        } else {
            error_code(reason)
        };
        self.profile_failed = false;
        state.finish(code);
        self.disconnecting = false;
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
                    self.credits = info.acl_packets;
                    crate::println!(
                        "Bluetooth HCI ready: ACL MTU {} bytes, {} transmit buffers",
                        info.acl_mtu,
                        info.acl_packets
                    );
                    let mut state = self.shared.state.lock();
                    state.adapter = info.address;
                    state.flags = abi::READY;
                    state.error = 0;
                }
                Action::PeerFound(peer) => {
                    self.shared.state.lock().update_peer(peer);
                    self.identify_profile(now);
                }
                Action::InquiryComplete(result) => self
                    .shared
                    .state
                    .lock()
                    .finish(result.err().map_or(0, error_code)),
                Action::LinkKey {
                    address,
                    key,
                    key_type,
                } => self.shared.state.lock().set_key(address, key, key_type),
                Action::LinkUp { address, handle } => {
                    {
                        let mut state = self.shared.state.lock();
                        state.peer = address;
                        state.flags |= abi::CONNECTED | abi::ENCRYPTED;
                    }
                    self.link = Some(Link {
                        handle,
                        channels: None,
                        joycon: None,
                        deadline: Some(now.saturating_add(PROFILE_TIMEOUT_MS)),
                    });
                    self.identify_profile(now);
                }
                Action::LinkDown { reason, .. } | Action::ConnectionFailed { reason, .. } => {
                    self.link_ended(reason)
                }
                Action::AclCredits { handle, packets } => {
                    if self.link.as_ref().is_some_and(|l| l.handle == handle) {
                        self.credits = self
                            .credits
                            .saturating_add(packets)
                            .min(self.controller.acl_packets);
                    }
                }
                Action::ControllerFault(_) => return Err("Bluetooth controller fault"),
            }
        }
        if self.disconnecting || self.profile_failed {
            return Ok(());
        }
        if self
            .link
            .as_ref()
            .and_then(|l| l.deadline)
            .is_some_and(|due| now >= due)
        {
            if self.link.as_ref().is_some_and(|l| l.joycon.is_some()) {
                self.fail_profile();
            } else {
                // A generic encrypted peer can be used without a recognized input profile.
                self.link.as_mut().unwrap().deadline = None;
                self.shared.state.lock().finish(0);
            }
            return Ok(());
        }
        if let Some(link) = &mut self.link
            && let (Some(channels), Some(joycon)) = (&mut link.channels, &mut link.joycon)
        {
            if !drive_hid(channels, joycon, now) {
                self.fail_profile();
                return Ok(());
            }
            if joycon.is_ready() && link.deadline.take().is_some() {
                let mut state = self.shared.state.lock();
                state.flags |= abi::PROFILE_READY;
                state.finish(0);
            }
            while self.credits != 0 {
                let Some(fragment) = channels.poll_acl(usize::from(self.controller.acl_mtu)) else {
                    break;
                };
                let hf = fragment.handle | u16::from(fragment.pb) << 12;
                let mut frame = Vec::with_capacity(5 + fragment.data.len());
                frame.push(h4::ACL);
                frame.extend_from_slice(&hf.to_le_bytes());
                frame.extend_from_slice(&(fragment.data.len() as u16).to_le_bytes());
                frame.extend_from_slice(&fragment.data);
                transport.send(&frame)?;
                self.credits -= 1;
            }
        }
        Ok(())
    }

    pub fn next_deadline(&self) -> Option<u64> {
        if self.repoll {
            return Some(0);
        }
        let link = self.link.as_ref();
        let hid_deadline = link
            .filter(|l| l.channels.as_ref().is_some_and(|c| c.is_ready()))
            .and_then(|l| l.joycon.as_ref()?.next_deadline_ms());
        [
            self.classic.next_deadline_ms(),
            link.and_then(|l| l.deadline),
            hid_deadline,
        ]
        .into_iter()
        .flatten()
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
