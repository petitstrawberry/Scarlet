//! Standalone protocol transcripts. Run rustc --edition=2024 --test classic.rs.
use super::*;

const PAD: Address = [1, 2, 3, 4, 5, 6];
const OTHER: Address = [7, 8, 9, 10, 11, 12];
const KEY: [u8; 16] = [0x50; 16];
const HANDLE: u16 = 0x123;

struct Host {
    sm: Classic,
    now: u64,
}

impl Host {
    fn new() -> Self {
        Self {
            sm: Classic::new(),
            now: 0,
        }
    }

    fn event(&mut self, code: u8, p: &[u8]) {
        self.sm.on_event(self.now, code, p).unwrap();
    }

    fn poll(&mut self) -> Option<Action> {
        self.sm.poll(self.now)
    }

    fn cmd(&mut self, opcode: u16) -> Command {
        let Some(Action::Command(cmd)) = self.poll() else {
            panic!("expected command {opcode:#06x}")
        };
        assert_eq!(cmd.opcode, opcode);
        cmd
    }

    fn status(&mut self, op: u16, status: u8) {
        let [lo, hi] = op.to_le_bytes();
        self.event(0x0f, &[status, 1, lo, hi]);
    }

    fn complete(&mut self, op: u16, credits: u8, result: &[u8]) {
        let mut event = [0; 255];
        event[0] = credits;
        event[1..3].copy_from_slice(&op.to_le_bytes());
        event[3..3 + result.len()].copy_from_slice(result);
        self.event(0x0e, &event[..3 + result.len()]);
    }

    fn addressed_ack(&mut self, op: u16, addr: Address) {
        let mut result = [0; 7];
        result[1..].copy_from_slice(&addr);
        self.complete(op, 1, &result);
    }

    fn init(&mut self) {
        assert_eq!(self.sm.connect(PAD), Err(Error::NotReady));
        for (op, result) in [
            (RESET, &[0][..]),
            (LOCAL_ADDR, &[0, 16, 16, 16, 16, 16, 16]),
            (BUFFER_SIZE, &[0, 0xfd, 3, 0, 8, 0, 0, 0]),
            (EVENT_MASK, &[0]),
            (SSP_MODE, &[0]),
        ] {
            let cmd = self.cmd(op);
            match op {
                SSP_MODE => assert_eq!(cmd.params(), &[1]),
                EVENT_MASK => {
                    let mask = u64::from_le_bytes(cmd.params().try_into().unwrap());
                    for ev in [2, 3, 5, 6, 8, 0x17, 0x18, 0x22, 0x2f, 0x31, 0x33, 0x36] {
                        assert_ne!(mask & (1 << (ev - 1)), 0);
                    }
                }
                _ => assert!(cmd.params().is_empty()),
            }
            assert!(self.poll().is_none());
            self.complete(op, 1, result);
        }
        assert_eq!(self.cmd(INQUIRY_MODE).params(), &[2]);
    }

    fn ready() -> Self {
        let mut h = Self::new();
        h.init();
        h.complete(INQUIRY_MODE, 1, &[0]);
        let Some(Action::Ready(info)) = h.poll() else {
            panic!()
        };
        assert_eq!(
            (info.address, info.acl_mtu, info.acl_packets),
            ([16; 6], 1021, 8)
        );
        assert_eq!(h.sm.next_deadline_ms(), None);
        h
    }

    fn connected(&mut self, addr: Address, handle: u16) {
        let mut p = [0; 11];
        p[1..3].copy_from_slice(&handle.to_le_bytes());
        p[3..9].copy_from_slice(&addr);
        p[9] = 1;
        self.event(3, &p);
    }

    fn acl(&mut self, bonded: bool) {
        if bonded {
            self.sm.provide_link_key(PAD, KEY);
            self.sm.reconnect(PAD).unwrap();
        } else {
            self.sm.connect(PAD).unwrap();
        }
        assert_eq!(
            self.cmd(CREATE).params(),
            &[1, 2, 3, 4, 5, 6, 0x18, 0xcc, 2, 0, 0, 0, 1]
        );
        self.status(CREATE, 0);
        self.connected(PAD, HANDLE);
        assert_eq!(self.cmd(AUTHENTICATE).params(), &HANDLE.to_le_bytes());
        self.status(AUTHENTICATE, 0);
    }

    fn new_key(&mut self, addr: Address, kind: u8) {
        let mut p = [0; 23];
        p[..6].copy_from_slice(&addr);
        p[6..22].copy_from_slice(&KEY);
        p[22] = kind;
        self.event(0x18, &p);
    }

    fn secure(&mut self) {
        self.event(6, &[0, 0x23, 1]);
        assert_eq!(self.cmd(ENCRYPT).params(), &[0x23, 1, 1]);
        self.status(ENCRYPT, 0);
        assert!(self.poll().is_none());
        self.event(8, &[0, 0x23, 1, 2]);
    }

    fn name(&mut self, addr: Address, name: &[u8]) {
        let mut p = [0; 255];
        p[1..7].copy_from_slice(&addr);
        p[7..7 + name.len()].copy_from_slice(name);
        self.event(7, &p);
    }

    fn peer(&mut self) -> Peer {
        let Some(Action::PeerFound(peer)) = self.poll() else {
            panic!()
        };
        peer
    }

    fn scan(&mut self) {
        self.sm.start_inquiry().unwrap();
        assert_eq!(self.cmd(INQUIRY).params(), &[0x33, 0x8b, 0x9e, 8, 0]);
        self.status(INQUIRY, 0);
    }
}

#[test]
fn initialization_falls_back_for_inquiry_mode_and_fails_on_mandatory_command_error() {
    let mut h = Host::new();
    h.init();
    h.complete(INQUIRY_MODE, 1, &[0x11]);
    assert_eq!(h.cmd(INQUIRY_MODE).params(), &[1]);
    h.complete(INQUIRY_MODE, 1, &[0x01]);
    assert!(matches!(h.poll(), Some(Action::Ready(_))));

    let mut h = Host::new();
    h.cmd(RESET);
    h.complete(RESET, 1, &[0x03]);
    assert!(matches!(
        h.poll(),
        Some(Action::ControllerFault(Reason::Hci(3)))
    ));
    assert!(h.poll().is_none());
}

#[test]
fn command_credits_are_absolute_and_timeouts_cover_stalled_commands_and_credits() {
    let mut h = Host::new();
    h.now = 100;
    h.cmd(RESET);
    assert_eq!(h.sm.next_deadline_ms(), Some(100 + COMMAND_MS));
    h.complete(LOCAL_ADDR, 7, &[0]);
    assert!(h.poll().is_none());
    h.complete(RESET, 0, &[0]);
    h.now += 10;
    assert!(h.poll().is_none());
    assert_eq!(h.sm.next_deadline_ms(), Some(110 + COMMAND_MS));
    h.complete(0, 2, &[]);
    h.cmd(LOCAL_ADDR);
    assert_eq!(
        h.sm.on_event(h.now, 0x0e, &[2, 9, 0x10, 0]),
        Err(Error::MalformedEvent)
    );
    assert!(h.poll().is_none());
    h.now += COMMAND_MS;
    assert!(matches!(
        h.poll(),
        Some(Action::ControllerFault(Reason::Timeout))
    ));

    let mut h = Host::new();
    h.complete(0, 0, &[]);
    assert!(h.poll().is_none());
    h.now = COMMAND_MS;
    assert!(matches!(
        h.poll(),
        Some(Action::ControllerFault(Reason::Timeout))
    ));
    assert_eq!(h.sm.next_deadline_ms(), None);
}

#[test]
fn discovery_decodes_multiple_records_rssi_eir_and_name_completion() {
    let mut h = Host::ready();
    h.scan();
    let mut r = [0; 29];
    r[0] = 2;
    for (slot, addr) in r[1..].chunks_exact_mut(14).zip([PAD, OTHER]) {
        slot[..6].copy_from_slice(&addr);
        slot[6] = 1;
        slot[9..12].copy_from_slice(&[8, 0x25, 0]);
        slot[12..14].copy_from_slice(&[0x34, 0x12]);
    }
    h.event(2, &r);
    for addr in [PAD, OTHER] {
        let p = h.peer();
        assert_eq!(
            (p.address, p.class_of_device, p.clock_offset, p.rssi),
            (addr, [8, 0x25, 0], 0x9234, None)
        );
    }
    let mut eir = [0; 255];
    eir[0] = 1;
    eir[1..7].copy_from_slice(&PAD);
    eir[7] = 2;
    eir[9..12].copy_from_slice(&[4, 5, 6]);
    eir[12..14].copy_from_slice(&[0x55, 0x22]);
    eir[14] = (-30i8) as u8;
    h.event(0x22, &eir[..15]);
    let p = h.peer();
    assert_eq!(
        (p.address, p.class_of_device, p.rssi),
        (PAD, [4, 5, 6], Some(-30))
    );
    // Shortened local name "S", followed by the complete local name "Sensor".
    eir[15..26].copy_from_slice(b"\x02\x08S\x07\x09Sensor");
    h.event(0x2f, &eir);
    assert_eq!(h.peer().name.unwrap().as_bytes(), b"Sensor");
    h.event(1, &[0]);
    assert_eq!(
        h.cmd(REMOTE_NAME).params(),
        &[7, 8, 9, 10, 11, 12, 1, 0, 0x34, 0x92]
    );
    h.status(REMOTE_NAME, 0);
    h.name(PAD, b"unrelated");
    assert!(h.poll().is_none());
    h.name(OTHER, b"Speaker\0ignored");
    let mut named = false;
    let mut done = false;
    while let Some(a) = h.poll() {
        match a {
            Action::InquiryComplete(Ok(())) => done = true,
            Action::PeerFound(p) => {
                assert_eq!(p.address, OTHER);
                assert_eq!(p.name.unwrap().as_bytes(), b"Speaker");
                named = true;
            }
            _ => panic!(),
        }
    }
    assert!(done && named);
}

#[test]
fn explicit_pairing_stages_key_until_authenticated_encryption() {
    let mut h = Host::ready();
    h.acl(false);
    h.event(0x17, &PAD);
    assert_eq!(h.cmd(NO_KEY).params(), &PAD);
    h.addressed_ack(NO_KEY, PAD);
    h.event(0x31, &PAD);
    assert_eq!(h.cmd(IO_REPLY).params(), &[1, 2, 3, 4, 5, 6, 3, 0, 4]);
    h.addressed_ack(IO_REPLY, PAD);
    let mut confirm = [0; 10];
    confirm[..6].copy_from_slice(&PAD);
    h.event(0x33, &confirm);
    assert_eq!(h.cmd(CONFIRM).params(), &PAD);
    h.addressed_ack(CONFIRM, PAD);
    h.new_key(PAD, 4);
    assert!(h.poll().is_none());
    h.secure();
    assert!(matches!(
        h.poll(),
        Some(Action::LinkKey {
            address: PAD,
            key: KEY,
            key_type: 4
        })
    ));
    assert!(matches!(
        h.poll(),
        Some(Action::LinkUp {
            address: PAD,
            handle: HANDLE
        })
    ));
    h.cmd(REMOTE_NAME);
    h.status(REMOTE_NAME, 0x1a);
    h.event(0x13, &[2, 7, 0, 5, 0, 0x23, 1, 3, 0]);
    assert!(matches!(
        h.poll(),
        Some(Action::AclCredits {
            handle: HANDLE,
            packets: 3
        })
    ));
    h.event(8, &[0, 0x23, 1, 1]);
    h.event(8, &[0, 7, 0, 0]);
    assert!(h.poll().is_none());
    h.event(8, &[0, 0x23, 1, 0]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            handle: HANDLE,
            reason: Reason::EncryptionLost,
            ..
        })
    ));
    assert_eq!(h.cmd(DISCONNECT).params(), &[0x23, 1, 0x13]);
    h.status(DISCONNECT, 0);
    h.event(5, &[0, 0x23, 1, 0x16]);
    assert!(h.poll().is_none());
    assert_eq!(h.sm.reconnect(PAD), Err(Error::MissingKey));
}

#[test]
fn unseen_bonded_peer_resolves_name_and_reconnect_refuses_fresh_pairing() {
    let mut h = Host::ready();
    assert_eq!(h.sm.reconnect(PAD), Err(Error::MissingKey));
    h.acl(true);
    h.event(0x31, &PAD);
    assert_eq!(h.cmd(NO_IO).params(), &[1, 2, 3, 4, 5, 6, 0x18]);
    h.addressed_ack(NO_IO, PAD);
    h.event(0x17, &PAD);
    assert_eq!(&h.cmd(KEY_REPLY).params()[6..], &KEY);
    h.addressed_ack(KEY_REPLY, PAD);
    h.secure();
    assert!(matches!(
        h.poll(),
        Some(Action::LinkUp {
            address: PAD,
            handle: HANDLE
        })
    ));
    assert_eq!(h.cmd(REMOTE_NAME).params(), &[1, 2, 3, 4, 5, 6, 2, 0, 0, 0]);
    h.status(REMOTE_NAME, 0);
    h.name(PAD, b"Desk Sensor");
    let p = h.peer();
    assert_eq!(
        (p.address, p.name.unwrap().as_bytes()),
        (PAD, b"Desk Sensor".as_slice())
    );
    assert!(h.poll().is_none());
    assert_eq!(h.sm.next_deadline_ms(), None);
    h.event(5, &[0, 0x23, 1, 0x08]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            reason: Reason::Hci(8),
            ..
        })
    ));
    h.sm.reconnect(PAD).unwrap();
}

#[test]
fn unsolicited_acl_and_security_events_do_not_establish_trust() {
    let mut h = Host::ready();
    h.sm.provide_link_key(OTHER, KEY);
    let mut request = [0; 10];
    request[..6].copy_from_slice(&OTHER);
    request[9] = 1;
    h.event(4, &request);
    assert_eq!(h.cmd(REJECT).params(), &[7, 8, 9, 10, 11, 12, 0x0f]);
    h.status(REJECT, 0);
    h.event(0x17, &OTHER);
    assert_eq!(h.cmd(NO_KEY).params(), &OTHER);
    h.addressed_ack(NO_KEY, OTHER);
    h.connected(OTHER, 77);
    assert_eq!(h.cmd(DISCONNECT).params(), &[77, 0, 0x13]);
    h.status(DISCONNECT, 0);
    h.acl(false);
    h.new_key(OTHER, 4);
    h.event(0x33, &request);
    assert_eq!(h.cmd(NO_CONFIRM).params(), &OTHER);
    h.addressed_ack(NO_CONFIRM, OTHER);
    assert!(h.poll().is_none());
    h.new_key(PAD, 3);
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            reason: Reason::UnsupportedKey,
            ..
        })
    ));
    h.cmd(DISCONNECT);
}

#[test]
fn page_timeout_cancels_and_quarantines_a_late_connection() {
    let mut h = Host::ready();
    h.sm.connect(PAD).unwrap();
    h.cmd(CREATE);
    h.status(CREATE, 0);
    assert_eq!(h.sm.next_deadline_ms(), Some(PAGE_MS));
    h.now = PAGE_MS;
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            address: PAD,
            reason: Reason::Timeout
        })
    ));
    assert_eq!(h.cmd(CREATE_CANCEL).params(), &PAD);
    h.addressed_ack(CREATE_CANCEL, PAD);
    assert_eq!(h.sm.connect(OTHER), Err(Error::Busy));
    h.connected(PAD, HANDLE);
    h.cmd(DISCONNECT);
    h.status(DISCONNECT, 0);
    h.event(5, &[0, 0x23, 1, 0x13]);
    assert!(h.poll().is_none());
    h.sm.connect(OTHER).unwrap();
}

#[test]
fn disconnect_revokes_queued_consent_and_opaque_authentication_cannot_open_link() {
    let mut h = Host::ready();
    h.acl(false);
    h.complete(0, 0, &[]);
    h.event(0x31, &PAD);
    assert!(h.poll().is_none());
    h.sm.disconnect().unwrap();
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            reason: Reason::LocalDisconnect,
            ..
        })
    ));
    h.complete(0, 1, &[]);
    h.cmd(DISCONNECT);

    let mut h = Host::ready();
    h.acl(false);
    h.secure();
    assert!(h.poll().is_none());
    h.now = h.sm.next_deadline_ms().unwrap();
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            reason: Reason::Timeout,
            ..
        })
    ));
    h.cmd(DISCONNECT);
}

#[test]
fn user_disconnect_wakes_worker_and_waits_for_controller_confirmation() {
    let mut h = Host::ready();
    h.acl(false);
    h.event(0x36, &[0, 1, 2, 3, 4, 5, 6]);
    h.secure();
    assert!(matches!(h.poll(), Some(Action::LinkUp { .. })));
    h.cmd(REMOTE_NAME);
    h.status(REMOTE_NAME, 0);
    h.name(PAD, b"Headphones");
    h.peer();
    assert_eq!(h.sm.next_deadline_ms(), None);
    h.sm.disconnect().unwrap();
    assert_eq!(h.sm.next_deadline_ms(), Some(0));
    h.cmd(DISCONNECT);
    h.status(DISCONNECT, 0);
    assert_eq!(h.sm.connect(PAD), Err(Error::Busy));
    assert!(h.poll().is_none());
    h.event(5, &[0, 0x23, 1, 0x16]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            address: PAD,
            handle: HANDLE,
            ..
        })
    ));
    h.sm.connect(OTHER).unwrap();
    h.cmd(CREATE);
}

#[test]
fn selecting_during_inquiry_or_name_lookup_cancels_discovery_before_paging() {
    for lookup in [false, true] {
        let mut h = Host::ready();
        h.scan();
        h.event(0x22, &[1, 1, 2, 3, 4, 5, 6, 1, 0, 0, 0, 0, 0x11, 0x22, 200]);
        h.peer();
        if lookup {
            h.event(1, &[0]);
            h.cmd(REMOTE_NAME);
            h.status(REMOTE_NAME, 0);
        }
        h.sm.connect(PAD).unwrap();
        if lookup {
            h.cmd(NAME_CANCEL);
            h.addressed_ack(NAME_CANCEL, PAD);
            assert!(h.poll().is_none());
            h.event(7, &[2, 1, 2, 3, 4, 5, 6]);
        } else {
            h.cmd(INQUIRY_CANCEL);
            h.complete(INQUIRY_CANCEL, 1, &[0]);
        }
        assert!(matches!(h.poll(), Some(Action::InquiryComplete(Ok(())))));
        assert_eq!(
            h.cmd(CREATE).params(),
            &[1, 2, 3, 4, 5, 6, 0x18, 0xcc, 1, 0, 0x11, 0xa2, 1]
        );
    }
}

#[test]
fn failed_create_status_resolves_a_racing_disconnect_without_a_phantom_page() {
    let mut h = Host::ready();
    h.sm.connect(PAD).unwrap();
    h.cmd(CREATE);
    h.sm.disconnect().unwrap();
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            reason: Reason::LocalDisconnect,
            ..
        })
    ));
    h.status(CREATE, 0x0c);
    assert!(h.poll().is_none());
    assert_eq!(h.sm.next_deadline_ms(), None);
    h.sm.connect(PAD).unwrap();
}

#[test]
fn bounded_inquiry_validates_records_and_reports_name_before_completion() {
    let mut h = Host::ready();
    h.scan();
    assert_eq!(h.sm.on_event(0, 0x22, &[2, 0]), Err(Error::MalformedEvent));
    assert_eq!(h.sm.on_event(0, 0x2f, &[1, 0]), Err(Error::MalformedEvent));
    let mut batch = [0; 1 + 14 * 9];
    batch[0] = 9;
    for (i, record) in batch[1..].chunks_exact_mut(14).enumerate() {
        record[..6].fill(i as u8);
    }
    h.event(0x22, &batch);
    for i in 0..8 {
        assert_eq!(h.peer().address, [i; 6]);
    }
    assert!(h.poll().is_none());
    h.event(1, &[0]);
    for i in 0..8 {
        let addr = [i; 6];
        assert_eq!(&h.cmd(REMOTE_NAME).params()[..6], &addr);
        h.status(REMOTE_NAME, 0);
        if i == 7 {
            h.name(addr, &[b'A'; 248]);
            let p = h.peer();
            assert_eq!(p.name.unwrap().as_bytes(), &[b'A'; 64]);
        } else {
            let mut failure = [i; 7];
            failure[0] = 4;
            h.event(7, &failure);
        }
    }
    assert!(matches!(h.poll(), Some(Action::InquiryComplete(Ok(())))));
}

#[test]
fn stale_rejection_cannot_disconnect_a_reused_handle() {
    let mut h = Host::ready();
    h.complete(0, 0, &[]);
    h.connected(OTHER, HANDLE);
    assert!(h.poll().is_none());
    h.event(5, &[0, 0x23, 1, 0x13]);
    h.complete(0, 1, &[]);
    assert!(h.poll().is_none());
    h.acl(false);
}

#[test]
fn authentication_failure_discards_staged_key_and_closes_acl() {
    let mut h = Host::ready();
    h.acl(false);
    h.new_key(PAD, 8);
    h.event(6, &[5, 0x23, 1]);
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            address: PAD,
            reason: Reason::Hci(5)
        })
    ));
    h.cmd(DISCONNECT);
    h.status(DISCONNECT, 0);
    h.event(5, &[0, 0x23, 1, 0x13]);
    assert!(h.poll().is_none());
    assert_eq!(h.sm.reconnect(PAD), Err(Error::MissingKey));
}

const SECOND_HANDLE: u16 = 0x456;
const SECOND_KEY: [u8; 16] = [0xa7; 16];

impl Host {
    fn expect_key(&mut self, addr: Address, key: [u8; 16]) {
        let reply = self.cmd(KEY_REPLY);
        assert_eq!(reply.params()[..6], addr);
        assert_eq!(reply.params()[6..], key);
        self.addressed_ack(KEY_REPLY, addr);
    }

    fn bond_up(&mut self, addr: Address, handle: u16, key: [u8; 16]) {
        self.sm.provide_link_key(addr, key);
        self.sm.reconnect(addr).unwrap();
        assert_eq!(self.cmd(CREATE).params()[..6], addr);
        self.status(CREATE, 0);
        self.connected(addr, handle);
        assert_eq!(self.cmd(AUTHENTICATE).params(), handle.to_le_bytes());
        self.status(AUTHENTICATE, 0);
        self.event(0x17, &addr);
        self.expect_key(addr, key);
        let [lo, hi] = handle.to_le_bytes();
        self.event(6, &[0, lo, hi]);
        assert_eq!(self.cmd(ENCRYPT).params(), &[lo, hi, 1]);
        self.status(ENCRYPT, 0);
        self.event(8, &[0, lo, hi, 1]);
        assert!(
            matches!(self.poll(), Some(Action::LinkUp { address, handle: h }) if (address, h) == (addr, handle))
        );
    }

    fn resolve_name(&mut self, addr: Address, name: &[u8]) {
        assert_eq!(self.cmd(REMOTE_NAME).params()[..6], addr);
        self.status(REMOTE_NAME, 0);
        self.name(addr, name);
    }

    fn expect_credits(&mut self, expected: u16, count: u16) {
        assert!(
            matches!(self.poll(), Some(Action::AclCredits { handle, packets }) if (handle, packets) == (expected, count))
        );
    }

    fn two_named_peers() -> Self {
        let mut h = Self::ready();
        h.bond_up(PAD, HANDLE, KEY);
        h.resolve_name(PAD, b"Mouse");
        assert_eq!(h.peer().address, PAD);
        h.bond_up(OTHER, SECOND_HANDLE, SECOND_KEY);
        h.resolve_name(OTHER, b"Keyboard");
        assert_eq!(h.peer().address, OTHER);
        h
    }
}

#[test]
fn two_encrypted_peers_use_addressed_keys_shared_command_credits_and_individual_acl_counts() {
    let mut h = Host::two_named_peers();
    assert_eq!(h.sm.disconnect(), Err(Error::Busy));
    assert_eq!(h.sm.connect(PAD), Err(Error::Busy));
    assert_eq!(h.sm.disconnect_peer([99; 6]), Err(Error::UnknownPeer));

    h.event(0x17, &OTHER);
    h.event(0x17, &PAD);
    let first = h.cmd(KEY_REPLY);
    assert_eq!(first.params()[..6], OTHER);
    assert_eq!(first.params()[6..], SECOND_KEY);
    assert!(h.poll().is_none());
    assert_eq!(
        h.sm.on_event(h.now, 0x0e, &[1, 0x0b, 4, 0, 1, 2, 3, 4, 5, 6]),
        Err(Error::MalformedEvent)
    );
    assert!(h.poll().is_none());
    let mut result = [0; 7];
    result[1..].copy_from_slice(&OTHER);
    h.complete(KEY_REPLY, 0, &result);
    assert!(h.poll().is_none());
    h.complete(0, 2, &[]);
    h.expect_key(PAD, KEY);

    h.new_key(OTHER, 6);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkKey {
            address: OTHER,
            key: KEY,
            key_type: 6
        })
    ));
    // An overflowing count for the second handle must not partially credit the first.
    assert_eq!(
        h.sm.on_event(
            h.now,
            0x13,
            &[3, 0x23, 1, 3, 0, 0x56, 4, 255, 255, 0x56, 4, 1, 0]
        ),
        Err(Error::MalformedEvent)
    );
    assert!(h.poll().is_none());
    h.event(0x13, &[3, 0x56, 4, 2, 0, 0x23, 1, 3, 0, 0x56, 4, 1, 0]);
    h.expect_credits(HANDLE, 3);
    h.expect_credits(SECOND_HANDLE, 3);

    h.sm.disconnect_peer(OTHER).unwrap();
    assert_eq!(h.sm.next_deadline_ms(), Some(0));
    assert_eq!(h.cmd(DISCONNECT).params(), &[0x56, 4, 0x13]);
    h.sm.disconnect_peer(OTHER).unwrap();
    h.status(DISCONNECT, 0);
    assert!(h.poll().is_none());
    h.event(5, &[0, 0x56, 4, 0x16]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            address: OTHER,
            handle: SECOND_HANDLE,
            reason: Reason::LocalDisconnect,
        })
    ));
    assert_eq!(h.sm.disconnect_peer(OTHER), Err(Error::UnknownPeer));
    h.event(0x17, &OTHER);
    assert_eq!(h.cmd(NO_KEY).params(), &OTHER);
    h.addressed_ack(NO_KEY, OTHER);
    h.event(0x17, &PAD);
    h.expect_key(PAD, KEY);
    h.event(0x13, &[2, 0x56, 4, 2, 0, 0x23, 1, 1, 0]);
    h.expect_credits(HANDLE, 1);
    assert!(h.poll().is_none());
}

#[test]
fn encryption_loss_and_teardown_timeout_isolate_the_handle_until_physical_disconnect() {
    let mut h = Host::two_named_peers();
    h.event(8, &[0, 0x23, 1, 0]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            address: PAD,
            handle: HANDLE,
            reason: Reason::EncryptionLost
        })
    ));
    assert_eq!(h.cmd(DISCONNECT).params(), &[0x23, 1, 0x13]);
    h.status(DISCONNECT, 0);
    h.event(0x13, &[2, 0x23, 1, 1, 0, 0x56, 4, 2, 0]);
    h.expect_credits(HANDLE, 1);
    h.expect_credits(SECOND_HANDLE, 2);
    h.now = h.sm.next_deadline_ms().unwrap();
    assert!(h.poll().is_none());
    assert_eq!(h.sm.next_deadline_ms(), None);
    assert_eq!(h.sm.reconnect(PAD), Err(Error::Busy));
    h.event(0x17, &PAD);
    assert_eq!(h.cmd(NO_KEY).params(), &PAD);
    h.addressed_ack(NO_KEY, PAD);
    h.event(0x13, &[1, 0x23, 1, 1, 0]);
    h.expect_credits(HANDLE, 1);
    h.event(0x17, &OTHER);
    h.expect_key(OTHER, SECOND_KEY);
    h.event(5, &[0, 0x23, 1, 0x13]);
    assert!(h.poll().is_none());
    h.sm.reconnect(PAD).unwrap();
    assert_eq!(h.cmd(CREATE).params()[..6], PAD);
}

#[test]
fn pairing_deadline_and_failed_command_ack_do_not_revoke_another_peers_key_reply() {
    let mut h = Host::ready();
    h.bond_up(PAD, HANDLE, KEY);
    h.resolve_name(PAD, b"Mouse");
    h.peer();
    h.sm.connect(OTHER).unwrap();
    h.cmd(CREATE);
    h.status(CREATE, 0);
    h.connected(OTHER, SECOND_HANDLE);
    h.cmd(AUTHENTICATE);
    h.status(AUTHENTICATE, 0);
    h.new_key(OTHER, 4);
    h.now = PAIR_MS - 1;
    h.event(0x31, &OTHER);
    h.cmd(IO_REPLY);
    h.event(0x17, &PAD);
    h.now = PAIR_MS;
    assert!(matches!(
        h.poll(),
        Some(Action::ConnectionFailed {
            address: OTHER,
            reason: Reason::Timeout
        })
    ));
    assert!(h.poll().is_none());
    h.complete(IO_REPLY, 1, &[0x0c]);
    h.expect_key(PAD, KEY);
    assert_eq!(h.cmd(DISCONNECT).params(), &[0x56, 4, 0x13]);
    h.status(DISCONNECT, 0x0c);
    h.event(0x17, &PAD);
    h.expect_key(PAD, KEY);
    h.event(5, &[0, 0x56, 4, 0x13]);
    assert!(h.poll().is_none());
    assert_eq!(h.sm.reconnect(OTHER), Err(Error::MissingKey));
}

#[test]
fn preempted_link_name_resumes_and_cannot_name_or_disconnect_the_other_peer() {
    let mut h = Host::ready();
    h.bond_up(PAD, HANDLE, KEY);
    assert_eq!(h.cmd(REMOTE_NAME).params()[..6], PAD);
    h.status(REMOTE_NAME, 0);
    h.sm.connect(OTHER).unwrap();
    assert_eq!(h.cmd(NAME_CANCEL).params(), &PAD);
    h.addressed_ack(NAME_CANCEL, PAD);
    h.event(7, &[2, 1, 2, 3, 4, 5, 6]);
    assert_eq!(h.cmd(CREATE).params()[..6], OTHER);
    h.status(CREATE, 0);
    h.connected(OTHER, SECOND_HANDLE);
    h.cmd(AUTHENTICATE);
    h.status(AUTHENTICATE, 0);
    h.new_key(OTHER, 4);
    h.event(6, &[0, 0x56, 4]);
    h.cmd(ENCRYPT);
    h.status(ENCRYPT, 0);
    h.event(8, &[0, 0x56, 4, 1]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkKey { address: OTHER, .. })
    ));
    assert!(matches!(
        h.poll(),
        Some(Action::LinkUp {
            address: OTHER,
            handle: SECOND_HANDLE
        })
    ));
    h.resolve_name(PAD, b"Mouse");
    assert_eq!(h.cmd(REMOTE_NAME).params()[..6], OTHER);
    h.status(REMOTE_NAME, 0);
    let p = h.peer();
    assert_eq!(
        (p.address, p.name.unwrap().as_bytes()),
        (PAD, b"Mouse".as_slice())
    );
    h.name(PAD, b"Late");
    h.event(5, &[0, 0x23, 1, 8]);
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown { address: PAD, .. })
    ));
    assert!(h.poll().is_none());
    h.name(OTHER, b"Keyboard");
    let p = h.peer();
    assert_eq!(
        (p.address, p.name.unwrap().as_bytes()),
        (OTHER, b"Keyboard".as_slice())
    );
}

#[test]
fn full_link_table_preserves_active_keys_and_controller_fault_ends_every_live_acl() {
    let mut h = Host::ready();
    for id in 1..=MAX_LINKS as u8 {
        h.bond_up([id; 6], u16::from(id), [id; 16]);
        h.resolve_name([id; 6], b"Known");
        h.peer();
    }
    assert_eq!(h.sm.connect([42; 6]), Err(Error::Busy));
    for id in 11..=22 {
        h.sm.provide_link_key([id; 6], [id; 16]);
    }
    for id in 1..=MAX_LINKS as u8 {
        h.event(0x17, &[id; 6]);
        h.expect_key([id; 6], [id; 16]);
    }
    h.scan();
    h.event(1, &[0]);
    assert!(matches!(h.poll(), Some(Action::InquiryComplete(Ok(())))));
    h.event(0x10, &[7]);
    for id in 1..=MAX_LINKS as u8 {
        assert!(
            matches!(h.poll(), Some(Action::LinkDown { address, handle, reason: Reason::HardwareError(7) })
            if address == [id; 6] && handle == u16::from(id))
        );
    }
    assert!(matches!(
        h.poll(),
        Some(Action::ControllerFault(Reason::HardwareError(7)))
    ));
    assert_eq!(h.sm.disconnect_peer(PAD), Err(Error::NotReady));
}

#[test]
fn local_close_timeout_is_per_peer_and_addressed_retry_waits_for_physical_completion() {
    let mut h = Host::two_named_peers();
    h.sm.disconnect_peer(PAD).unwrap();
    h.cmd(DISCONNECT);
    h.status(DISCONNECT, 0);
    h.now = h.sm.next_deadline_ms().unwrap();
    assert!(matches!(
        h.poll(),
        Some(Action::LinkDown {
            address: PAD,
            handle: HANDLE,
            reason: Reason::Timeout
        })
    ));
    assert!(h.poll().is_none());
    h.sm.disconnect_peer(PAD).unwrap();
    assert_eq!(h.cmd(DISCONNECT).params(), &[0x23, 1, 0x13]);
    h.status(DISCONNECT, 0);
    h.event(0x13, &[1, 0x23, 1, 2, 0]);
    h.expect_credits(HANDLE, 2);
    h.event(5, &[0, 0x23, 1, 0x16]);
    assert!(h.poll().is_none());
    h.event(0x17, &OTHER);
    h.expect_key(OTHER, SECOND_KEY);
}
