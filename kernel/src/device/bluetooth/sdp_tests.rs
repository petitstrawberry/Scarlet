use super::super::{CloseReason, DEFAULT_MTU, Event, L2capSession};
use super::*;
use alloc::vec;

fn variable(kind: u8, bytes: &[u8]) -> Vec<u8> {
    let mut out = if bytes.len() <= 255 {
        vec![kind << 3 | 5, bytes.len() as u8]
    } else {
        let n = u16::try_from(bytes.len()).unwrap();
        vec![kind << 3 | 6, (n >> 8) as u8, n as u8]
    };
    out.extend_from_slice(bytes);
    out
}
fn seq(entries: &[Vec<u8>]) -> Vec<u8> {
    variable(6, &entries.concat())
}
fn num(n: u16) -> Vec<u8> {
    vec![9, (n >> 8) as u8, n as u8]
}
fn uuid(n: u16) -> Vec<u8> {
    vec![0x19, (n >> 8) as u8, n as u8]
}
fn protocol(psm: u16) -> Vec<u8> {
    seq(&[seq(&[uuid(0x100), num(psm)]), seq(&[uuid(0x11)])])
}
fn hid_record(control: u16, interrupt: u16, descriptor: &[u8]) -> Vec<u8> {
    seq(&[
        num(1),
        seq(&[uuid(HID)]),
        num(4),
        protocol(control),
        num(0x0d),
        seq(&[protocol(interrupt)]),
        num(0x206),
        seq(&[seq(&[vec![8, 0x22], variable(4, descriptor)])]),
    ])
}
fn identity(source: u16, primary: bool) -> Vec<u8> {
    seq(&[
        num(1),
        seq(&[uuid(PNP)]),
        num(0x201),
        num(0x1234),
        num(0x202),
        num(0x5678),
        num(0x203),
        num(0x0100),
        num(0x204),
        vec![0x28, primary as u8],
        num(0x205),
        num(source),
    ])
}
fn response(id: u16, part: &[u8], token: &[u8]) -> Vec<u8> {
    let mut out = vec![7];
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(3 + part.len() as u16 + token.len() as u16).to_be_bytes());
    out.extend_from_slice(&(part.len() as u16).to_be_bytes());
    out.extend_from_slice(part);
    out.push(token.len() as u8);
    out.extend_from_slice(token);
    out
}

fn query(id: u16, class: u16, token: &[u8]) -> Vec<u8> {
    let mut out = vec![
        6,
        (id >> 8) as u8,
        id as u8,
        0,
        15 + token.len() as u8,
        0x35,
        3,
        0x19,
        (class >> 8) as u8,
        class as u8,
        0x02,
        0x88,
        0x35,
        5,
        0x0a,
        0,
        0,
        0xff,
        0xff,
        token.len() as u8,
    ];
    out.extend_from_slice(token);
    out
}

#[test]
fn arbitrary_sdp_continuations_preserve_report_descriptor_and_primary_id_namespace() {
    let descriptor = [0xf1; 350];
    let hid = seq(&[hid_record(0x1001, 0x1003, &descriptor)]);
    let pnp = seq(&[identity(1, false), identity(2, true)]);
    for split in 2..=hid.len() - 2 {
        let mut client = Client::default();
        assert_eq!(client.request(DEFAULT_MTU, &[]), query(1, HID, &[]));
        let token = &[0x00, 0xfe, 0x84];
        let Step::Request(next) = client
            .response(&response(1, &hid[..split], token), DEFAULT_MTU)
            .unwrap()
        else {
            panic!("missing continuation")
        };
        assert_eq!(next, query(2, HID, token));
        assert!(matches!(
            client.response(&response(1, &hid[..split], token), DEFAULT_MTU),
            Ok(Step::Wait)
        ));
        let Step::Request(next) = client
            .response(&response(2, &hid[split..], &[]), DEFAULT_MTU)
            .unwrap()
        else {
            panic!("missing Device ID query")
        };
        assert_eq!(next, query(3, PNP, &[]));
        let Step::Request(next) = client
            .response(&response(3, &pnp[..7], &[5]), DEFAULT_MTU)
            .unwrap()
        else {
            panic!("missing Device ID continuation")
        };
        assert_eq!(next, query(4, PNP, &[5]));
        let Step::Done(Some(service)) = client
            .response(&response(4, &pnp[7..], &[]), DEFAULT_MTU)
            .unwrap()
        else {
            panic!("missing HID service")
        };
        assert_eq!(
            (service.control_psm, service.interrupt_psm),
            (0x1001, 0x1003)
        );
        assert_eq!(service.report_descriptor, descriptor);
        assert_eq!(
            service.device_id,
            Some(DeviceId {
                vendor_id_source: 2,
                vendor_id: 0x1234,
                product_id: 0x5678,
                version: 0x100
            })
        );
    }
}

#[test]
fn only_a_matching_hid_service_enables_hid_and_identity_is_optional() {
    let hid = seq(&[hid_record(0x11, 0x13, &[0x05, 1])]);
    for empty in [seq(&[]), seq(&[seq(&[num(1), seq(&[uuid(0x1101)])])])] {
        let mut client = Client::default();
        client.request(DEFAULT_MTU, &[]);
        assert!(matches!(
            client.response(&response(1, &empty, &[]), DEFAULT_MTU),
            Ok(Step::Done(None))
        ));
    }
    for pnp in [response(2, &seq(&[]), &[]), vec![1, 0, 2, 0, 2, 0, 3]] {
        let mut client = Client::default();
        client.request(DEFAULT_MTU, &[]);
        assert!(matches!(
            client.response(&response(1, &hid, &[]), DEFAULT_MTU),
            Ok(Step::Request(_))
        ));
        let Step::Done(Some(service)) = client.response(&pnp, DEFAULT_MTU).unwrap() else {
            panic!("missing HID service")
        };
        assert!(service.device_id.is_none());
    }
}

#[test]
fn ambiguous_or_invalid_device_id_never_supplies_a_driver_identity() {
    let hid = seq(&[hid_record(0x11, 0x13, &[0x05, 1])]);
    for (records, malformed) in [
        (seq(&[identity(1, false), identity(2, false)]), false),
        (seq(&[identity(1, true), identity(2, true)]), true),
        (seq(&[identity(3, true)]), true),
    ] {
        let mut client = Client::default();
        client.request(DEFAULT_MTU, &[]);
        client
            .response(&response(1, &hid, &[]), DEFAULT_MTU)
            .unwrap();
        let result = client.response(&response(2, &records, &[]), DEFAULT_MTU);
        if malformed {
            assert!(result.is_err());
        } else {
            assert!(matches!(result, Ok(Step::Done(Some(service))) if service.device_id.is_none()));
        }
    }
}

#[test]
fn malformed_records_and_bounded_continuations_cannot_start_hid() {
    let invalid_protocol = seq(&[
        num(1),
        seq(&[uuid(HID)]),
        num(4),
        seq(&[seq(&[uuid(0x100), num(0x11)])]),
    ]);
    let bad = [
        response(1, &seq(&[invalid_protocol]), &[]),
        response(1, &seq(&[hid_record(0x11, 0x11, &[1])]), &[]),
        response(1, &seq(&[hid_record(0x1, 0x13, &[1])]), &[]),
        response(1, &seq(&[hid_record(0x11, 0x13, &[])]), &[]),
        response(
            1,
            &seq(&[seq(&[num(1), seq(&[uuid(HID)]), num(1), seq(&[uuid(HID)])])]),
            &[],
        ),
        response(1, &[0x37, 0xff, 0xff, 0xff, 0xff], &[]),
        response(1, &[0x35, 0], &[1; 17]),
        response(1, &[0x35, 0, 0], &[]),
        vec![7, 0, 1, 0, 5, 0, 2, 0x35, 0, 1],
        vec![1, 0, 1, 0, 2, 0, 3],
    ];
    for packet in bad {
        let mut client = Client::default();
        client.request(DEFAULT_MTU, &[]);
        assert!(client.response(&packet, DEFAULT_MTU).is_err());
    }
    for bytes in [2, 640] {
        let mut client = Client::default();
        client.request(DEFAULT_MTU, &[]);
        let last = MAX_RESPONSES.min(MAX_ATTRIBUTE_BYTES / bytes + 1);
        for id in 1..last {
            assert!(matches!(
                client.response(&response(id as u16, &vec![0; bytes], &[1]), DEFAULT_MTU),
                Ok(Step::Request(_))
            ));
        }
        assert!(
            client
                .response(&response(last as u16, &vec![0; bytes], &[1]), DEFAULT_MTU)
                .is_err()
        );
    }
}

fn l2cap_pdu(cid: u16, data: &[u8]) -> Vec<u8> {
    [
        &(data.len() as u16).to_le_bytes()[..],
        &cid.to_le_bytes(),
        data,
    ]
    .concat()
}
fn signal(session: &mut L2capSession, code: u8, id: u8, params: &[u16]) {
    let params: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
    let body = [
        &[code, id][..],
        &(params.len() as u16).to_le_bytes(),
        &params,
    ]
    .concat();
    session.receive_acl(2, &l2cap_pdu(1, &body)).unwrap();
}
fn pop_signal(session: &mut L2capSession, code: u8, params: &[u16]) -> u8 {
    let packet = session.poll_acl(1024).unwrap().data;
    assert_eq!(&packet[2..5], &[1, 0, code]);
    assert_eq!(
        &packet[8..],
        params
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    );
    packet[5]
}
const LOCAL: u16 = 0x42;
const PEER: u16 = 0x87;

fn configured_sdp(handle: u16) -> L2capSession {
    let mut s = L2capSession::discover(handle, DEFAULT_MTU).unwrap();
    let id = pop_signal(&mut s, 2, &[1, LOCAL]);
    signal(&mut s, 3, id, &[PEER, LOCAL, 0, 0]);
    let id = pop_signal(&mut s, 4, &[PEER, 0, 0x0201, DEFAULT_MTU]);
    signal(&mut s, 5, id, &[LOCAL, 0, 0]);
    signal(&mut s, 4, 66, &[LOCAL, 0]);
    assert_eq!(pop_signal(&mut s, 5, &[PEER, 0, 0]), 66);
    assert_eq!(
        s.poll_acl(1024).unwrap().data,
        l2cap_pdu(PEER, &query(1, HID, &[]))
    );
    assert!(s.poll_event().is_none());
    assert!(!s.is_ready());
    s
}

#[test]
fn simultaneous_acl_discovery_closes_before_publishing_and_uses_discovered_psms() {
    let mut hid = configured_sdp(5);
    let mut other = configured_sdp(6);
    // Neither SDP session accepts an inbound HID connection during discovery.
    signal(&mut other, 2, 8, &[0x11, 0x99]);
    assert_eq!(pop_signal(&mut other, 3, &[0, 0x99, 2, 0]), 8);
    let descriptor = [0x06, 0, 0xff, 0x09, 1];
    let records = seq(&[hid_record(0x1001, 0x1003, &descriptor)]);
    let yes = l2cap_pdu(LOCAL, &response(1, &records, &[]));
    let no = l2cap_pdu(LOCAL, &response(1, &seq(&[]), &[]));
    hid.receive_acl(2, &yes[..3]).unwrap();
    other.receive_acl(2, &no[..2]).unwrap();
    hid.receive_acl(1, &yes[3..]).unwrap();
    other.receive_acl(1, &no[2..]).unwrap();
    let id = pop_signal(&mut other, 6, &[PEER, LOCAL]);
    assert!(other.poll_event().is_none());
    signal(&mut other, 7, id, &[PEER, LOCAL]);
    assert_eq!(other.poll_event(), Some(Event::Discovered(None)));
    assert!(other.poll_acl(1024).is_none());
    assert_eq!(
        hid.poll_acl(1024).unwrap().data,
        l2cap_pdu(PEER, &query(2, PNP, &[]))
    );
    hid.receive_acl(
        2,
        &l2cap_pdu(LOCAL, &response(2, &seq(&[identity(1, true)]), &[])),
    )
    .unwrap();
    let id = pop_signal(&mut hid, 6, &[PEER, LOCAL]);
    signal(&mut hid, 7, id.wrapping_add(1), &[PEER, LOCAL]);
    assert!(hid.poll_event().is_none());
    signal(&mut hid, 7, id, &[PEER, LOCAL]);
    let Some(Event::Discovered(Some(service))) = hid.poll_event() else {
        panic!("discovery incomplete")
    };
    assert_eq!(service.device_id.unwrap().vendor_id_source, 1);
    assert_eq!(service.report_descriptor, descriptor);
    let mut hid =
        L2capSession::for_hid(5, DEFAULT_MTU, service.control_psm, service.interrupt_psm).unwrap();
    hid.start().unwrap();
    for (psm, local, peer) in [(0x1001, 0x40, 0x90), (0x1003, 0x41, 0x91)] {
        let id = pop_signal(&mut hid, 2, &[psm, local]);
        signal(&mut hid, 3, id, &[peer, local, 0, 0]);
        let id = pop_signal(&mut hid, 4, &[peer, 0, 0x0201, DEFAULT_MTU]);
        signal(&mut hid, 5, id, &[local, 0, 0]);
        signal(&mut hid, 4, 88, &[local, 0, 0x0201, 100]);
        pop_signal(&mut hid, 5, &[peer, 0, 0, 0x0201, 100]);
    }
    assert_eq!(hid.poll_event(), Some(Event::Ready));
    assert_eq!(hid.interrupt_tx_mtu(), Some(100));
}

#[test]
fn canceled_or_malformed_discovery_releases_only_its_sdp_channel() {
    let mut unsent = L2capSession::discover(1, DEFAULT_MTU).unwrap();
    unsent.disconnect().unwrap();
    assert!(unsent.poll_acl(1024).is_none());
    assert_eq!(unsent.poll_event(), Some(Event::Closed(CloseReason::Local)));
    let mut pending = L2capSession::discover(2, DEFAULT_MTU).unwrap();
    let id = pop_signal(&mut pending, 2, &[1, LOCAL]);
    pending.disconnect().unwrap();
    assert!(pending.poll_event().is_none());
    signal(&mut pending, 3, id, &[PEER, LOCAL, 0, 0]);
    let id = pop_signal(&mut pending, 6, &[PEER, LOCAL]);
    signal(&mut pending, 7, id, &[PEER, LOCAL]);
    assert_eq!(
        pending.poll_event(),
        Some(Event::Closed(CloseReason::Local))
    );
    let mut bad = configured_sdp(5);
    bad.receive_acl(2, &l2cap_pdu(LOCAL, &[7, 0, 1, 0, 0]))
        .unwrap();
    let id = pop_signal(&mut bad, 6, &[PEER, LOCAL]);
    assert!(bad.poll_event().is_none());
    signal(&mut bad, 7, id, &[PEER, LOCAL]);
    assert_eq!(
        bad.poll_event(),
        Some(Event::Closed(CloseReason::MalformedSdp))
    );

    let mut closing = configured_sdp(5);
    closing
        .receive_acl(2, &l2cap_pdu(LOCAL, &response(1, &seq(&[]), &[])))
        .unwrap();
    let id = pop_signal(&mut closing, 6, &[PEER, LOCAL]);
    closing.disconnect().unwrap();
    signal(&mut closing, 7, id, &[PEER, LOCAL]);
    assert_eq!(
        closing.poll_event(),
        Some(Event::Closed(CloseReason::Local))
    );
}
