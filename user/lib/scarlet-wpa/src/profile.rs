//! Bounded single-network WPA2-Personal/CCMP configuration. No secret Debug output.
use crate::{Error, derive_pmk};
use alloc::vec::Vec;
use zeroize::Zeroizing;
pub struct Profile {
    pub ssid: Vec<u8>,
    pub pmk: Zeroizing<[u8; 32]>,
}
// Keep partial PMK bytes in wiping storage even when hex decoding fails.
fn unhex(value: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    if value.len() % 2 != 0 {
        return Err(Error::InvalidProfile);
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(value.len() / 2));
    for pair in value.as_bytes().chunks_exact(2) {
        let text = core::str::from_utf8(pair).map_err(|_| Error::InvalidProfile)?;
        bytes.push(u8::from_str_radix(text, 16).map_err(|_| Error::InvalidProfile)?);
    }
    Ok(bytes)
}
fn quoted(value: &str) -> Option<&str> {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .filter(|v| !v.contains('\\') && !v.contains('"'))
}
impl Profile {
    pub fn parse(input: &str) -> Result<Self, Error> {
        if input.len() > 4096 {
            return Err(Error::InvalidProfile);
        }
        let mut inside = false;
        let mut ended = false;
        let mut opened = false;
        let mut ssid = None;
        let mut psk = None;
        let mut options = 0u8;
        for raw in input.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "}" {
                if !inside {
                    return Err(Error::InvalidProfile);
                }
                inside = false;
                ended = true;
                continue;
            }
            let (name, value) = line.split_once('=').ok_or(Error::InvalidProfile)?;
            let name = name.trim();
            let value = value.trim();
            if name == "network" && value == "{" {
                if opened || ended {
                    return Err(Error::InvalidProfile);
                }
                opened = true;
                inside = true;
                continue;
            }
            if !inside {
                return Err(Error::InvalidProfile);
            }
            match name {
                "ssid" => {
                    if ssid.is_some() {
                        return Err(Error::InvalidProfile);
                    }
                    ssid = Some(if let Some(s) = quoted(value) {
                        s.as_bytes().to_vec()
                    } else {
                        unhex(value)?.to_vec()
                    });
                }
                "psk" => {
                    if psk.is_some() {
                        return Err(Error::InvalidProfile);
                    }
                    psk = Some(value);
                }
                "key_mgmt" | "proto" | "pairwise" | "group" => {
                    let (bit, expected) = match name {
                        "key_mgmt" => (1, "WPA-PSK"),
                        "proto" => (2, "RSN"),
                        "pairwise" => (4, "CCMP"),
                        _ => (8, "CCMP"),
                    };
                    if options & bit != 0 || value != expected {
                        return Err(Error::InvalidProfile);
                    }
                    options |= bit;
                }
                _ => return Err(Error::InvalidProfile),
            }
        }
        if inside || !ended {
            return Err(Error::InvalidProfile);
        }
        let ssid = ssid.ok_or(Error::InvalidProfile)?;
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(Error::InvalidProfile);
        }
        let value = psk.ok_or(Error::InvalidProfile)?;
        let pmk = if let Some(pass) = quoted(value) {
            Zeroizing::new(derive_pmk(pass, &ssid).map_err(|_| Error::InvalidProfile)?)
        } else {
            if value.len() != 64 {
                return Err(Error::InvalidProfile);
            }
            let bytes = unhex(value)?;
            let mut pmk = Zeroizing::new([0; 32]);
            pmk.copy_from_slice(&bytes);
            pmk
        };
        Ok(Self { ssid, pmk })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_profile_and_strict_policy() {
        let p = Profile::parse(
            "network={\nssid=\"IEEE\"\npsk=\"password\"\nproto=RSN\npairwise=CCMP\n}\n",
        )
        .unwrap();
        assert_eq!(p.ssid, b"IEEE");
        assert_eq!(*p.pmk, derive_pmk("password", b"IEEE").unwrap());
        for bad in [
            "network={\nssid=\"IEEE\"\npsk=\"password\"\npairwise=TKIP\n}",
            "network={\nssid=\"IEEE\"\npsk=\"short\"\n}",
            "network={\nssid=\"IEEE\"\nssid=\"IEEE\"\npsk=\"password\"\n}",
        ] {
            assert!(Profile::parse(bad).is_err());
        }
    }
}
