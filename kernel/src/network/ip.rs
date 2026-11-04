//! Protocol-neutral IP address values.
//!
//! IPv4 and IPv6 headers remain in their protocol-specific modules. This file
//! provides the common address surface used by routing and socket code, plus a
//! canonical representation for IPv4-mapped IPv6 addresses.

use core::fmt::{self, Write};
use core::str::FromStr;

use super::ipv4::Ipv4Address;

/// The number of bytes in a native IPv6 address.
pub const IPV6_ADDRESS_LEN: usize = 16;

/// An IPv4 or native IPv6 address.
///
/// This is an on-stack value. It deliberately does not turn an IPv6 socket into
/// an IPv4 socket: `V6` always retains all 128 bits, including IPv4-mapped forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpAddress {
    V4(Ipv4Address),
    V6(Ipv6Address),
}

/// The wire-size octets of an [`IpAddress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpAddressBytes {
    V4([u8; 4]),
    V6([u8; IPV6_ADDRESS_LEN]),
}

impl IpAddressBytes {
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::V4(bytes) => bytes,
            Self::V6(bytes) => bytes,
        }
    }

    pub const fn len(&self) -> usize {
        match self {
            Self::V4(_) => 4,
            Self::V6(_) => IPV6_ADDRESS_LEN,
        }
    }

    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl IpAddress {
    pub const fn from_v4(address: Ipv4Address) -> Self {
        Self::V4(address)
    }

    pub const fn from_v6(address: Ipv6Address) -> Self {
        Self::V6(address)
    }

    /// Parse either a 4-byte IPv4 address or a 16-byte IPv6 address.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes.len() {
            4 => Some(Self::V4(Ipv4Address::from_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
            ]))),
            IPV6_ADDRESS_LEN => {
                let mut octets = [0; IPV6_ADDRESS_LEN];
                octets.copy_from_slice(bytes);
                Some(Self::V6(Ipv6Address::from_bytes(octets)))
            }
            _ => None,
        }
    }

    pub fn to_bytes(self) -> IpAddressBytes {
        match self {
            Self::V4(address) => IpAddressBytes::V4(address.as_bytes()),
            Self::V6(address) => IpAddressBytes::V6(address.as_bytes()),
        }
    }

    pub const fn is_v4(self) -> bool {
        matches!(self, Self::V4(_))
    }

    pub const fn is_v6(self) -> bool {
        matches!(self, Self::V6(_))
    }

    pub const fn version(self) -> u8 {
        match self {
            Self::V4(_) => 4,
            Self::V6(_) => 6,
        }
    }

    pub const fn is_v4_mapped(self) -> bool {
        matches!(self, Self::V6(address) if address.is_ipv4_mapped())
    }

    pub const fn as_v4(self) -> Option<Ipv4Address> {
        match self {
            Self::V4(address) => Some(address),
            Self::V6(_) => None,
        }
    }

    pub const fn as_v6(self) -> Option<Ipv6Address> {
        match self {
            Self::V4(_) => None,
            Self::V6(address) => Some(address),
        }
    }

    /// Return the IPv4 address carried by a native `V4` value or by a canonical
    /// IPv4-mapped `V6` value. Other IPv6 values remain IPv6.
    pub fn as_v4_embedded(self) -> Option<Ipv4Address> {
        match self {
            Self::V4(address) => Some(address),
            Self::V6(address) => address.to_ipv4_mapped(),
        }
    }

    /// Fold an IPv4-mapped IPv6 value to `V4` for older IPv4-only protocols.
    /// Native IPv6 values are preserved unchanged.
    pub fn canonicalize_v4_mapped(self) -> Self {
        match self {
            Self::V6(address) => match address.to_ipv4_mapped() {
                Some(address) => Self::V4(address),
                None => self,
            },
            Self::V4(_) => self,
        }
    }

    /// Represent an IPv4 address, or an existing IPv4-mapped IPv6 address, in
    /// the canonical `::ffff:a.b.c.d/96` family representation.
    pub fn to_v4_mapped(self) -> Option<Self> {
        match self {
            Self::V4(address) => Some(Self::V6(Ipv6Address::from_ipv4_mapped(address))),
            Self::V6(address) if address.is_ipv4_mapped() => Some(self),
            Self::V6(_) => None,
        }
    }

    pub fn is_loopback(self) -> bool {
        match self {
            Self::V4(address) => address.is_loopback(),
            Self::V6(address) => address.is_loopback(),
        }
    }

    pub fn is_unspecified(self) -> bool {
        match self {
            Self::V4(address) => address.is_any(),
            Self::V6(address) => address.is_unspecified(),
        }
    }

    pub fn is_multicast(self) -> bool {
        match self {
            Self::V4(address) => (224..=239).contains(&address.as_bytes()[0]),
            Self::V6(address) => address.is_multicast(),
        }
    }
}

impl From<Ipv4Address> for IpAddress {
    fn from(address: Ipv4Address) -> Self {
        Self::V4(address)
    }
}

impl From<Ipv6Address> for IpAddress {
    fn from(address: Ipv6Address) -> Self {
        Self::V6(address)
    }
}

impl fmt::Display for IpAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V4(address) => {
                let octets = address.as_bytes();
                write!(
                    formatter,
                    "{}.{}.{}.{}",
                    octets[0], octets[1], octets[2], octets[3]
                )
            }
            Self::V6(address) => fmt::Display::fmt(address, formatter),
        }
    }
}

impl FromStr for IpAddress {
    type Err = IpAddressParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.contains(':') {
            Ipv6Address::from_str(value).map(Self::V6)
        } else {
            parse_ipv4(value).map(Self::V4)
        }
    }
}

/// An IPv6 address in network byte order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ipv6Address([u8; IPV6_ADDRESS_LEN]);

impl Ipv6Address {
    pub const UNSPECIFIED: Self = Self([0; IPV6_ADDRESS_LEN]);
    pub const LOOPBACK: Self = Self([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

    pub const fn new(a: u16, b: u16, c: u16, d: u16, e: u16, f: u16, g: u16, h: u16) -> Self {
        let a = a.to_be_bytes();
        let b = b.to_be_bytes();
        let c = c.to_be_bytes();
        let d = d.to_be_bytes();
        let e = e.to_be_bytes();
        let f = f.to_be_bytes();
        let g = g.to_be_bytes();
        let h = h.to_be_bytes();
        Self([
            a[0], a[1], b[0], b[1], c[0], c[1], d[0], d[1], e[0], e[1], f[0], f[1], g[0], g[1],
            h[0], h[1],
        ])
    }

    pub const fn from_bytes(bytes: [u8; IPV6_ADDRESS_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn from_segments(segments: [u16; 8]) -> Self {
        Self::new(
            segments[0],
            segments[1],
            segments[2],
            segments[3],
            segments[4],
            segments[5],
            segments[6],
            segments[7],
        )
    }

    pub const fn as_bytes(&self) -> [u8; IPV6_ADDRESS_LEN] {
        self.0
    }

    pub const fn segments(&self) -> [u16; 8] {
        [
            u16::from_be_bytes([self.0[0], self.0[1]]),
            u16::from_be_bytes([self.0[2], self.0[3]]),
            u16::from_be_bytes([self.0[4], self.0[5]]),
            u16::from_be_bytes([self.0[6], self.0[7]]),
            u16::from_be_bytes([self.0[8], self.0[9]]),
            u16::from_be_bytes([self.0[10], self.0[11]]),
            u16::from_be_bytes([self.0[12], self.0[13]]),
            u16::from_be_bytes([self.0[14], self.0[15]]),
        ]
    }

    pub const fn is_unspecified(&self) -> bool {
        matches!(self.0, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    }

    pub const fn is_loopback(&self) -> bool {
        matches!(self.0, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1])
    }

    pub const fn is_multicast(&self) -> bool {
        self.0[0] == 0xff
    }

    pub const fn is_link_local(&self) -> bool {
        self.0[0] == 0xfe && (self.0[1] & 0xc0) == 0x80
    }

    pub const fn is_unique_local(&self) -> bool {
        (self.0[0] & 0xfe) == 0xfc
    }

    /// Whether this address has the IPv4-mapped prefix `::ffff:0:0/96`.
    pub const fn is_ipv4_mapped(&self) -> bool {
        matches!(
            self.0,
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, _, _, _, _]
        )
    }

    pub fn to_ipv4_mapped(&self) -> Option<Ipv4Address> {
        if self.is_ipv4_mapped() {
            Some(Ipv4Address::from_bytes([
                self.0[12], self.0[13], self.0[14], self.0[15],
            ]))
        } else {
            None
        }
    }

    pub fn from_ipv4_mapped(address: Ipv4Address) -> Self {
        let octets = address.as_bytes();
        Self([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, octets[0], octets[1], octets[2], octets[3],
        ])
    }

    /// Return the first `prefix_len` bits as an address with the remaining bits
    /// cleared. `prefix_len` must be at most 128.
    pub fn masked(&self, prefix_len: u8) -> Option<Self> {
        let prefix_len = usize::from(prefix_len);
        if prefix_len > 128 {
            return None;
        }

        let mut octets = self.0;
        let full_bytes = prefix_len / 8;
        let remaining_bits = prefix_len % 8;
        if remaining_bits == 0 {
            octets[full_bytes..].fill(0);
        } else {
            octets[full_bytes] &= 0xff << (8 - remaining_bits);
            octets[full_bytes + 1..].fill(0);
        }
        Some(Self(octets))
    }

    pub fn contains(&self, other: Self, prefix_len: u8) -> bool {
        let Some(mask) = self.masked(prefix_len) else {
            return false;
        };
        other.masked(prefix_len) == Some(mask)
    }
}

impl fmt::Display for Ipv6Address {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ipv4) = self.to_ipv4_mapped() {
            let octets = ipv4.as_bytes();
            return write!(
                formatter,
                "::ffff:{}.{}.{}.{}",
                octets[0], octets[1], octets[2], octets[3]
            );
        }

        let segments = self.segments();
        let mut run_start = 0;
        let mut best_start = 0;
        let mut best_len = 0;
        while run_start < segments.len() {
            if segments[run_start] != 0 {
                run_start += 1;
                continue;
            }
            let mut run_len = 0;
            while run_start + run_len < segments.len() && segments[run_start + run_len] == 0 {
                run_len += 1;
            }
            if run_len > best_len {
                best_start = run_start;
                best_len = run_len;
            }
            run_start += run_len;
        }

        if best_len < 2 {
            for (index, segment) in segments.iter().enumerate() {
                if index != 0 {
                    formatter.write_char(':')?;
                }
                write!(formatter, "{segment:x}")?;
            }
            return Ok(());
        }

        for (index, segment) in segments[..best_start].iter().enumerate() {
            if index != 0 {
                formatter.write_char(':')?;
            }
            write!(formatter, "{segment:x}")?;
        }
        formatter.write_str("::")?;
        let suffix_start = best_start + best_len;
        for (index, segment) in segments[suffix_start..].iter().enumerate() {
            if index != 0 {
                formatter.write_char(':')?;
            }
            write!(formatter, "{segment:x}")?;
        }
        Ok(())
    }
}

impl FromStr for Ipv6Address {
    type Err = IpAddressParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.contains('%') {
            return Err(IpAddressParseError);
        }

        let mut segments = [0u16; 8];
        let (left, right) = match value.split_once("::") {
            Some((left, right)) if !right.contains("::") => (left, Some(right)),
            Some(_) => return Err(IpAddressParseError),
            None => (value, None),
        };
        if right.is_some() && left.contains('.') {
            return Err(IpAddressParseError);
        }

        let left_len = parse_ipv6_side(left, &mut segments, false)?;
        if let Some(right) = right {
            let mut right_segments = [0u16; 8];
            let right_len = parse_ipv6_side(right, &mut right_segments, true)?;
            if left_len + right_len >= 8 {
                return Err(IpAddressParseError);
            }
            let right_start = 8 - right_len;
            segments[right_start..].copy_from_slice(&right_segments[right_start..]);
        } else if left_len != 8 {
            return Err(IpAddressParseError);
        }

        Ok(Self::from_segments(segments))
    }
}

fn parse_ipv6_side(
    value: &str,
    output: &mut [u16; 8],
    align_to_end: bool,
) -> Result<usize, IpAddressParseError> {
    if value.is_empty() {
        return Ok(0);
    }

    let mut parsed = [0u16; 8];
    let mut count = 0;
    for (index, part) in value.split(':').enumerate() {
        if part.is_empty() || count >= 8 {
            return Err(IpAddressParseError);
        }
        if part.contains('.') {
            if index + 1 != value.split(':').count() {
                return Err(IpAddressParseError);
            }
            let ipv4 = parse_ipv4(part)?;
            let octets = ipv4.as_bytes();
            if count + 2 > 8 {
                return Err(IpAddressParseError);
            }
            parsed[count] = u16::from_be_bytes([octets[0], octets[1]]);
            parsed[count + 1] = u16::from_be_bytes([octets[2], octets[3]]);
            count += 2;
        } else {
            if part.len() > 4 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(IpAddressParseError);
            }
            parsed[count] = u16::from_str_radix(part, 16).map_err(|_| IpAddressParseError)?;
            count += 1;
        }
    }

    let output_start = if align_to_end { 8 - count } else { 0 };
    output[output_start..output_start + count].copy_from_slice(&parsed[..count]);
    Ok(count)
}

fn parse_ipv4(value: &str) -> Result<Ipv4Address, IpAddressParseError> {
    let mut octets = [0u8; 4];
    let mut count = 0;
    for part in value.split('.') {
        if count == 4
            || part.is_empty()
            || part.len() > 3
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(IpAddressParseError);
        }
        octets[count] = part.parse::<u8>().map_err(|_| IpAddressParseError)?;
        count += 1;
    }
    if count != 4 {
        return Err(IpAddressParseError);
    }
    Ok(Ipv4Address::from_bytes(octets))
}

/// Invalid textual IP address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpAddressParseError;

impl fmt::Display for IpAddressParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid IP address")
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    #[test_case]
    fn common_address_bytes_round_trip() {
        let v4 = IpAddress::from_v4(Ipv4Address::new(192, 0, 2, 1));
        assert_eq!(v4.to_bytes().as_slice(), &[192, 0, 2, 1]);
        assert_eq!(IpAddress::from_bytes(&[192, 0, 2, 1]), Some(v4));

        let v6 = IpAddress::from_v6(Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));
        assert_eq!(IpAddress::from_bytes(v6.to_bytes().as_slice()), Some(v6));
        assert_eq!(v6.to_bytes().len(), 16);
        assert_eq!(IpAddress::from_bytes(&[0; 15]), None);
    }

    #[test_case]
    fn ipv4_mapped_conversion_is_explicit_and_canonical() {
        let ipv4 = Ipv4Address::new(203, 0, 113, 7);
        let mapped = Ipv6Address::from_ipv4_mapped(ipv4);
        assert_eq!(IpAddress::from_v6(mapped).version(), 6);
        assert!(mapped.is_ipv4_mapped());
        assert_eq!(mapped.to_string(), "::ffff:203.0.113.7");
        assert_eq!(mapped.to_ipv4_mapped(), Some(ipv4));

        let address = IpAddress::from_v6(mapped);
        assert!(address.is_v6());
        assert_eq!(address.as_v4_embedded(), Some(ipv4));
        assert!(address.is_v4_mapped());
        assert_eq!(address.to_v4_mapped(), Some(address));
        assert_eq!(address.canonicalize_v4_mapped(), IpAddress::from_v4(ipv4));
        assert_eq!(
            IpAddress::from_v4(ipv4).to_v4_mapped(),
            Some(IpAddress::from_v6(mapped))
        );

        let mapped_loopback = IpAddress::from_v6(Ipv6Address::from_ipv4_mapped(Ipv4Address::new(
            127, 0, 0, 1,
        )));
        assert!(!mapped_loopback.is_loopback());
        assert!(mapped_loopback.canonicalize_v4_mapped().is_loopback());

        let native_v6 = Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        assert_eq!(
            IpAddress::from_v6(native_v6).canonicalize_v4_mapped(),
            IpAddress::from_v6(native_v6)
        );
    }

    #[test_case]
    fn ipv6_parser_and_formatter_cover_compression_and_embedded_ipv4() {
        let cases = [
            ("2001:db8:0:0:0:0:0:1", "2001:db8::1"),
            ("2001:0:0:1:0:0:0:1", "2001:0:0:1::1"),
            ("0:0:0:0:0:0:0:0", "::"),
            ("0:0:0:0:0:0:0:1", "::1"),
            ("1:0:0:2:0:0:0:3", "1:0:0:2::3"),
            ("::ffff:192.0.2.128", "::ffff:192.0.2.128"),
        ];
        for (input, canonical) in cases {
            let address = input.parse::<Ipv6Address>().unwrap();
            assert_eq!(address.to_string(), canonical);
            assert_eq!(canonical.parse::<Ipv6Address>().unwrap(), address);
        }
    }

    #[test_case]
    fn ipv6_parser_rejects_noncanonical_or_ambiguous_forms() {
        for input in [
            "",
            ":::1",
            "1:2:3:4:5:6:7",
            "1:2:3:4:5:6:7:8:9",
            "1::2::3",
            "fe80::1%eth0",
            "::192.0.2.1:1",
            "::192.0.2",
            "::ffff:192.0.2.01",
            "+1::",
            "00000::",
            "+1.2.3.4",
        ] {
            assert!(input.parse::<Ipv6Address>().is_err(), "accepted {input}");
        }
    }

    #[test_case]
    fn ip_address_text_parser_selects_family() {
        assert_eq!(
            "192.0.2.1".parse::<IpAddress>().unwrap(),
            IpAddress::from_v4(Ipv4Address::new(192, 0, 2, 1))
        );
        assert_eq!(
            "2001:db8::1".parse::<IpAddress>().unwrap(),
            IpAddress::from_v6(Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1))
        );
        assert!("192.0.2".parse::<IpAddress>().is_err());
    }

    #[test_case]
    fn prefix_mask_and_contains_use_the_requested_width() {
        let address = Ipv6Address::new(0x2001, 0xdb8, 0xabcd, 0x1234, 0, 0, 0, 7);
        assert_eq!(
            address.masked(48),
            Some(Ipv6Address::new(0x2001, 0xdb8, 0xabcd, 0, 0, 0, 0, 0))
        );
        assert_eq!(
            address.masked(53),
            Some(Ipv6Address::from_bytes([
                0x20, 0x01, 0x0d, 0xb8, 0xab, 0xcd, 0x10, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]))
        );
        assert!(Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0).contains(address, 32));
        assert_eq!(address.masked(129), None);
    }
}
