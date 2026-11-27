//! Classic SDP ServiceSearchAttribute discovery over one configured L2CAP channel.
//! SDP integers use network byte order, including the contents of data elements.
//! See Bluetooth Core, Vol 3, Part B, §§3–4, HID Profile and Device ID Profile.
extern crate alloc;
use self::alloc::vec::Vec;

const HID: u16 = 0x1124;
const PNP: u16 = 0x1200;
const MAX_ATTRIBUTE_BYTES: usize = 16 * 1024;
const MAX_RESPONSES: usize = 64;
const MAX_DESCRIPTOR: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceId {
    /// Bluetooth SIG (1) or USB-IF (2) vendor namespace.
    pub vendor_id_source: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidService {
    pub control_psm: u16,
    pub interrupt_psm: u16,
    pub report_descriptor: Vec<u8>,
    pub device_id: Option<DeviceId>,
}

#[derive(Default)]
pub(super) struct Client {
    transaction: u16,
    responses: usize,
    attributes: Vec<u8>,
    hid: Option<HidService>,
}

pub(super) enum Step {
    Wait,
    Request(Vec<u8>),
    Done(Option<HidService>),
}

impl Client {
    pub(super) fn request(&mut self, mtu: u16, continuation: &[u8]) -> Vec<u8> {
        let uuid = if self.hid.is_some() { PNP } else { HID };
        self.transaction += 1; // bounded by the per-query response limit
        let mut request = Vec::with_capacity(20 + continuation.len());
        request.push(0x06);
        request.extend_from_slice(&self.transaction.to_be_bytes());
        request.extend_from_slice(&(15 + continuation.len() as u16).to_be_bytes());
        request.extend_from_slice(&[0x35, 3, 0x19]);
        request.extend_from_slice(&uuid.to_be_bytes());
        // Reserve room for response header, byte count and the longest token.
        request.extend_from_slice(&(mtu - 24).to_be_bytes());
        request.extend_from_slice(&[0x35, 5, 0x0a, 0, 0, 0xff, 0xff]);
        request.push(continuation.len() as u8);
        request.extend_from_slice(continuation);
        request
    }

    pub(super) fn response(&mut self, packet: &[u8], mtu: u16) -> Result<Step, ()> {
        let (&kind, mut params) = packet.split_first().ok_or(())?;
        let transaction = uint16(&mut params)?;
        let length = uint16(&mut params)?;
        if usize::from(length) != params.len() {
            return Err(());
        }
        if transaction != self.transaction {
            return Ok(Step::Wait);
        }
        if kind == 1 && self.hid.is_some() {
            // The optional PnP query may be unavailable on an older SDP server.
            uint16(&mut params)?;
            return Ok(Step::Done(self.hid.take()));
        }
        if kind != 7 {
            return Err(());
        }
        let count = uint16(&mut params)?;
        if !(2..=mtu - 24).contains(&count) {
            return Err(());
        }
        let (fragment, tail) = params.split_at_checked(usize::from(count)).ok_or(())?;
        let (&length, token) = tail.split_first().ok_or(())?;
        self.responses += 1;
        if usize::from(length) != token.len()
            || token.len() > 16
            || self.attributes.len() + fragment.len() > MAX_ATTRIBUTE_BYTES
            || self.responses > MAX_RESPONSES
        {
            return Err(());
        }
        self.attributes.extend_from_slice(fragment);
        if !token.is_empty() {
            if self.responses == MAX_RESPONSES {
                return Err(());
            }
            return Ok(Step::Request(self.request(mtu, token)));
        }
        let bytes = core::mem::take(&mut self.attributes);
        self.responses = 0;
        let mut list = bytes.as_slice();
        let mut records = Element::take(&mut list)?.sequence()?;
        if !list.is_empty() {
            return Err(());
        }
        if let Some(mut hid) = self.hid.take() {
            let mut identity = None;
            let mut primary = None;
            let mut count = 0;
            while !records.is_empty() {
                let record = Record::take(&mut records)?;
                if record.has_class(PNP)? {
                    let (device, is_primary) = record.identity()?;
                    count += 1;
                    identity = Some(device);
                    if is_primary && primary.replace(device).is_some() {
                        return Err(());
                    }
                }
            }
            hid.device_id = primary.or(if count == 1 { identity } else { None });
            return Ok(Step::Done(Some(hid)));
        }
        while !records.is_empty() {
            let record = Record::take(&mut records)?;
            if record.has_class(HID)? && self.hid.is_none() {
                self.hid = Some(record.hid()?);
            }
        }
        if self.hid.is_some() {
            Ok(Step::Request(self.request(mtu, &[])))
        } else {
            Ok(Step::Done(None))
        }
    }
}

fn uint16(bytes: &mut &[u8]) -> Result<u16, ()> {
    let (value, rest) = bytes.split_first_chunk::<2>().ok_or(())?;
    *bytes = rest;
    Ok(u16::from_be_bytes(*value))
}

#[derive(Clone, Copy)]
struct Element<'a> {
    kind: u8,
    data: &'a [u8],
}

impl<'a> Element<'a> {
    fn take(bytes: &mut &'a [u8]) -> Result<Self, ()> {
        let (&header, mut tail) = bytes.split_first().ok_or(())?;
        let kind = header >> 3;
        let size = header & 7;
        let len = match (kind, size) {
            (0, 0) => 0,
            (1 | 2, 0..=4) | (3, 1 | 2 | 4) | (5, 0) => 1usize << size,
            (4 | 6 | 7 | 8, 5..=7) => {
                let (prefix, rest) = tail.split_at_checked(1 << (size - 5)).ok_or(())?;
                tail = rest;
                prefix.iter().fold(0usize, |n, b| n << 8 | usize::from(*b))
            }
            _ => return Err(()),
        };
        let (data, rest) = tail.split_at_checked(len).ok_or(())?;
        *bytes = rest;
        Ok(Self { kind, data })
    }

    fn sequence(self) -> Result<&'a [u8], ()> {
        (self.kind == 6).then_some(self.data).ok_or(())
    }

    fn number(self) -> Result<u16, ()> {
        if self.kind != 1 {
            return Err(());
        }
        Ok(u16::from_be_bytes(self.data.try_into().map_err(|_| ())?))
    }

    fn uuid(self, short: u16) -> bool {
        if self.kind != 3 {
            return false;
        }
        let u = u32::from(short).to_be_bytes();
        match self.data.len() {
            2 => self.data == short.to_be_bytes(),
            4 => self.data == u,
            16 => {
                self.data[..4] == u
                    && self.data[4..] == [0, 0, 0x10, 0, 0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb]
            }
            _ => false,
        }
    }
}

// Keep only the attributes consumed by the two profiles; validate the wire
// ordering and walk every pair without allocating one object per attribute.
struct Record<'a>(&'a [u8]);
impl<'a> Record<'a> {
    fn take(records: &mut &'a [u8]) -> Result<Self, ()> {
        let data = Element::take(records)?.sequence()?;
        let mut attrs = data;
        let mut previous = None;
        while !attrs.is_empty() {
            let id = Element::take(&mut attrs)?.number()?;
            if previous.is_some_and(|p| p >= id) {
                return Err(());
            }
            previous = Some(id);
            Element::take(&mut attrs)?;
        }
        Ok(Self(data))
    }

    fn get(&self, id: u16) -> Result<Option<Element<'a>>, ()> {
        let mut attrs = self.0;
        while !attrs.is_empty() {
            let key = Element::take(&mut attrs)?.number()?;
            let value = Element::take(&mut attrs)?;
            if key == id {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }

    fn require(&self, id: u16) -> Result<Element<'a>, ()> {
        self.get(id)?.ok_or(())
    }

    fn has_class(&self, uuid: u16) -> Result<bool, ()> {
        let mut list = self.require(1)?.sequence()?;
        let mut found = false;
        while !list.is_empty() {
            let item = Element::take(&mut list)?;
            if item.kind != 3 {
                return Err(());
            }
            found |= item.uuid(uuid);
        }
        Ok(found)
    }

    fn hid(&self) -> Result<HidService, ()> {
        let control_psm = hid_psm(self.require(4)?)?.ok_or(())?;
        let mut alternatives = self.require(0x0d)?.sequence()?;
        let mut interrupt_psm = None;
        while !alternatives.is_empty() {
            if let Some(psm) = hid_psm(Element::take(&mut alternatives)?)?
                && interrupt_psm.replace(psm).is_some()
            {
                return Err(());
            }
        }
        let interrupt_psm = interrupt_psm.ok_or(())?;
        if !valid_hid_psms(control_psm, interrupt_psm) {
            return Err(());
        }
        let mut descriptors = self.require(0x206)?.sequence()?;
        let mut report = None;
        while !descriptors.is_empty() {
            let mut entry = Element::take(&mut descriptors)?.sequence()?;
            let ty = Element::take(&mut entry)?;
            let text = Element::take(&mut entry)?;
            if ty.kind != 1 || ty.data.len() != 1 || text.kind != 4 || !entry.is_empty() {
                return Err(());
            }
            if ty.data == [0x22]
                && (text.data.is_empty()
                    || text.data.len() > MAX_DESCRIPTOR
                    || report.replace(text.data).is_some())
            {
                return Err(());
            }
        }
        Ok(HidService {
            control_psm,
            interrupt_psm,
            report_descriptor: report.ok_or(())?.to_vec(),
            device_id: None,
        })
    }

    fn identity(&self) -> Result<(DeviceId, bool), ()> {
        let vendor_id_source = self.require(0x205)?.number()?;
        if !matches!(vendor_id_source, 1 | 2) {
            return Err(());
        }
        let primary = match self.get(0x204)? {
            None => false,
            Some(Element { kind: 5, data: [v] }) => *v != 0,
            _ => return Err(()),
        };
        Ok((
            DeviceId {
                vendor_id_source,
                vendor_id: self.require(0x201)?.number()?,
                product_id: self.require(0x202)?.number()?,
                version: self.require(0x203)?.number()?,
            },
            primary,
        ))
    }
}

fn hid_psm(list: Element<'_>) -> Result<Option<u16>, ()> {
    let mut protocols = list.sequence()?;
    let mut transport = Element::take(&mut protocols)?.sequence()?;
    if !Element::take(&mut transport)?.uuid(0x0100) {
        return Ok(None);
    }
    let psm = Element::take(&mut transport)?.number()?;
    if !transport.is_empty() || protocols.is_empty() {
        return Ok(None);
    }
    let mut upper = Element::take(&mut protocols)?.sequence()?;
    Ok(
        (Element::take(&mut upper)?.uuid(0x0011) && upper.is_empty() && protocols.is_empty())
            .then_some(psm),
    )
}

pub(super) fn valid_hid_psms(control: u16, interrupt: u16) -> bool {
    fn valid(psm: u16, assigned: u16) -> bool {
        psm == assigned || (psm >= 0x1001 && psm & 0x0101 == 1)
    }
    control != interrupt && valid(control, 0x11) && valid(interrupt, 0x13)
}

#[cfg(all(test, not(target_os = "none")))]
#[path = "sdp_tests.rs"]
mod tests;
