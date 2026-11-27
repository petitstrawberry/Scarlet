//! ServiceSearchAttribute responses for the two simulated Classic HID devices.
use alloc::{vec, vec::Vec};

const HID: u16 = 0x1124;
const DEVICE_ID: u16 = 0x1200;

fn be(bytes: &[u8]) -> u16 {
    u16::from_be_bytes(bytes[..2].try_into().unwrap())
}

fn uint(value: u16) -> Vec<u8> {
    [&[0x09][..], &value.to_be_bytes()].concat()
}

fn uuid(value: u16) -> Vec<u8> {
    [&[0x19][..], &value.to_be_bytes()].concat()
}

fn sequence(elements: &[Vec<u8>]) -> Vec<u8> {
    let body = elements.concat();
    [&[0x35, body.len().try_into().unwrap()][..], &body].concat()
}

fn protocols(psm: u16) -> Vec<u8> {
    sequence(&[
        sequence(&[uuid(0x0100), uint(psm)]),
        sequence(&[uuid(0x0011)]),
    ])
}

use super::wire::DESCRIPTOR as REPORT_DESCRIPTOR;

fn attributes(service: u16, supported: bool) -> Vec<u8> {
    if !supported {
        assert_eq!(service, HID);
        return sequence(&[]);
    }
    let mut fields = vec![uint(0x0001), sequence(&[uuid(service)])];
    match service {
        HID => {
            let report = [
                &[0x25, REPORT_DESCRIPTOR.len() as u8][..],
                REPORT_DESCRIPTOR,
            ]
            .concat();
            fields.extend([
                uint(0x0004),
                protocols(0x0011),
                uint(0x000d),
                sequence(&[protocols(0x0013)]),
                uint(0x0206),
                sequence(&[sequence(&[vec![0x08, 0x22], report])]),
            ]);
        }
        DEVICE_ID => fields.extend([
            uint(0x0200),
            uint(0x0103),
            uint(0x0201),
            uint(0xf001),
            uint(0x0202),
            uint(0x1234),
            uint(0x0203),
            uint(0x0100),
            uint(0x0204),
            vec![0x28, 1],
            uint(0x0205),
            uint(2),
        ]),
        _ => panic!("unexpected SDP UUID {service:04x}"),
    }
    sequence(&[sequence(&fields)])
}

#[derive(Default)]
pub struct Discovery {
    offset: usize,
    hid_complete: bool,
    pub identity_complete: bool,
}

impl Discovery {
    pub fn response(&mut self, request: &[u8], supported: bool) -> Vec<u8> {
        assert_eq!(request[0], 6, "expected ServiceSearchAttributeRequest");
        assert_eq!(usize::from(be(&request[3..])), request.len() - 5);
        let body = &request[5..];
        assert_eq!(&body[..3], &[0x35, 3, 0x19]);
        let service = be(&body[3..]);
        assert_eq!(service, if self.hid_complete { DEVICE_ID } else { HID });
        assert!(!self.identity_complete);
        let maximum = usize::from(be(&body[5..]));
        assert!(maximum >= 7);
        assert_eq!(&body[7..14], &[0x35, 5, 0x0a, 0, 0, 0xff, 0xff]);
        if self.offset == 0 {
            assert_eq!(&body[14..], &[0]);
        } else {
            assert_eq!(&body[14..], &[1, self.offset as u8]);
        }

        let all = attributes(service, supported);
        let end = (self.offset + maximum.min(53)).min(all.len());
        let chunk = &all[self.offset..end];
        let mut params = (chunk.len() as u16).to_be_bytes().to_vec();
        params.extend_from_slice(chunk);
        if end < all.len() {
            self.offset = end;
            params.extend_from_slice(&[1, end.try_into().unwrap()]);
        } else {
            self.offset = 0;
            self.hid_complete = true;
            self.identity_complete = service == DEVICE_ID;
            params.push(0);
        }
        [
            &[7][..],
            &request[1..3],
            &(params.len() as u16).to_be_bytes(),
            &params,
        ]
        .concat()
    }
}
