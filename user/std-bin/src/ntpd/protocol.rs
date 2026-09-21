//! SNTP wire validation and offset calculation (RFC 4330 / RFC 5905).
//! No local wall-clock value is used: even an unset RTC can be synchronized.

const SECOND: u64 = 1_000_000_000;
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
pub const PACKET_LEN: usize = 48;
pub const MAX_ROUND_TRIP_NS: u64 = 5 * SECOND;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Kiss {
        code: [u8; 4],
        retry_after_secs: u64,
    },
}

#[derive(Debug)]
pub struct Sample {
    pub unix_ns: u64,
    pub monotonic_ns: u64,
    pub delay_ns: u64,
    pub stratum: u8,
}

/// The transmit field is an opaque, nonzero correlation cookie, echoed by the
/// server as its origin timestamp. This does not authenticate an NTP server.
pub fn request(cookie: [u8; 8]) -> [u8; PACKET_LEN] {
    let mut packet = [0; PACKET_LEN];
    packet[0] = (4 << 3) | 3; // NTPv4 client
    packet[2] = 10; // nominal poll 1024 seconds
    packet[3] = (-20i8) as u8;
    packet[40..48].copy_from_slice(&cookie);
    packet
}

fn timestamp(bytes: &[u8]) -> Result<u64, Error> {
    if bytes == [0; 8] {
        return Err(Error::Invalid("zero server timestamp"));
    }
    let seconds = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as u64;
    let fraction = u32::from_be_bytes(bytes[4..].try_into().unwrap()) as u64;
    // RFC 4330's 1968..2104 window: handle the February 2036 era rollover
    // without trusting the broken local clock to choose an era.
    let era = if seconds & 0x8000_0000 == 0 {
        1u64 << 32
    } else {
        0
    };
    let unix_seconds = (era + seconds)
        .checked_sub(NTP_UNIX_OFFSET)
        .ok_or(Error::Invalid("timestamp before Unix epoch"))?;
    Ok(unix_seconds * SECOND + ((fraction * SECOND) >> 32))
}

pub fn response(
    packet: &[u8],
    cookie: [u8; 8],
    sent_ns: u64,
    received_ns: u64,
) -> Result<Sample, Error> {
    if packet.len() < PACKET_LEN {
        return Err(Error::Invalid("short NTP response"));
    }
    let version = (packet[0] >> 3) & 7;
    if !(3..=4).contains(&version) || packet[0] & 7 != 4 {
        return Err(Error::Invalid("not an NTPv3/v4 server response"));
    }
    if cookie == [0; 8] || packet[24..32] != cookie {
        return Err(Error::Invalid("origin timestamp mismatch"));
    }
    if packet[1] == 0 {
        return Err(Error::Kiss {
            code: packet[12..16].try_into().unwrap(),
            retry_after_secs: 1u64 << (packet[2] as i8).clamp(4, 17),
        });
    }
    if packet[0] >> 6 == 3 || packet[1] > 15 {
        return Err(Error::Invalid("unsynchronized NTP server"));
    }
    let round_trip = received_ns
        .checked_sub(sent_ns)
        .filter(|delay| *delay <= MAX_ROUND_TRIP_NS)
        .ok_or(Error::Invalid("invalid or excessive round trip"))?;
    let receive = timestamp(&packet[32..40])?;
    let transmit = timestamp(&packet[40..48])?;
    let processing = transmit
        .checked_sub(receive)
        .filter(|elapsed| *elapsed <= round_trip + 1_000_000)
        .ok_or(Error::Invalid("inconsistent server timestamps"))?;
    let delay_ns = round_trip.saturating_sub(processing);
    Ok(Sample {
        unix_ns: transmit + delay_ns / 2,
        monotonic_ns: received_ns,
        delay_ns,
        stratum: packet[1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const COOKIE: [u8; 8] = *b"request1";
    const UNIX: u64 = 1_790_000_000;

    fn stamp(unix: u64, fraction: u32) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&((unix + NTP_UNIX_OFFSET) as u32).to_be_bytes());
        bytes[4..].copy_from_slice(&fraction.to_be_bytes());
        bytes
    }

    fn packet() -> [u8; 48] {
        let mut p = [0; 48];
        p[0] = (4 << 3) | 4;
        p[1] = 2;
        p[24..32].copy_from_slice(&COOKIE);
        p[32..40].copy_from_slice(&stamp(UNIX, 0));
        p[40..48].copy_from_slice(&stamp(UNIX, 1 << 29)); // 125 ms processing
        p
    }

    #[test]
    fn corrects_large_rtc_error_using_only_monotonic_elapsed_time() {
        let sample = response(&packet(), COOKIE, 100, 375_000_100).unwrap();
        assert_eq!(sample.unix_ns, UNIX * SECOND + 250_000_000);
        assert_eq!(sample.monotonic_ns, 375_000_100);
        assert_eq!(sample.delay_ns, 250_000_000);
        assert_eq!(sample.stratum, 2);
    }

    #[test]
    fn era_rollover_and_fraction_are_decoded_without_rtc() {
        assert_eq!(
            timestamp(&stamp(2_085_978_495, u32::MAX)).unwrap(),
            2_085_978_495_999_999_999
        );
        assert_eq!(
            timestamp(&stamp(2_085_978_496, 1 << 31)).unwrap(),
            2_085_978_496_500_000_000
        );
        assert!(timestamp(&[0; 8]).is_err());
        assert!(timestamp(&[0x80, 0, 0, 0, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn rejects_malformed_stale_and_unsynchronized_responses() {
        let good = packet();
        for length in 0..48 {
            assert!(response(&good[..length], COOKIE, 0, 400_000_000).is_err());
        }
        for (offset, value) in [(0, 0x23), (0, 0xe4), (0, 0x14), (1, 16), (24, 0)] {
            let mut p = good;
            p[offset] = value;
            assert!(response(&p, COOKIE, 0, 400_000_000).is_err());
        }
        assert!(response(&good, [0; 8], 0, 400_000_000).is_err());
        assert!(response(&good, COOKIE, 1, 0).is_err());
        assert!(response(&good, COOKIE, 0, MAX_ROUND_TRIP_NS + 1).is_err());
        assert!(response(&good, COOKIE, 0, 10_000_000).is_err());
        let mut reversed = good;
        reversed[32..40].copy_from_slice(&stamp(UNIX + 1, 0));
        assert!(response(&reversed, COOKIE, 0, 400_000_000).is_err());
    }

    #[test]
    fn kiss_of_death_requires_matching_request_and_carries_poll_interval() {
        let mut p = packet();
        p[1] = 0;
        p[2] = 11;
        p[12..16].copy_from_slice(b"RATE");
        assert_eq!(
            response(&p, COOKIE, 0, 400_000_000).unwrap_err(),
            Error::Kiss {
                code: *b"RATE",
                retry_after_secs: 2048,
            }
        );
        p[24] = 0;
        assert!(matches!(
            response(&p, COOKIE, 0, 400_000_000),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn request_and_v3_response_are_compatible() {
        let request = request(COOKIE);
        assert_eq!(request[0], 0x23);
        assert_eq!(&request[40..], &COOKIE);
        let mut p = packet();
        p[0] = (3 << 3) | 4;
        assert!(response(&p, COOKIE, 0, 400_000_000).is_ok());
    }
}
