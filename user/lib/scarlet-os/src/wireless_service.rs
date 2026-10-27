//! Bounded local control protocol for wirelessd. Credentials are request-only.
extern crate alloc;
use crate::socket::Socket;
use alloc::{string::String, vec, vec::Vec};
pub const SOCKET: &str = "/tmp/wirelessd.sock";
pub const STATUS: u8 = 1;
pub const SCAN: u8 = 2;
pub const CONNECT: u8 = 3;
pub const DISCONNECT: u8 = 4;
pub const FORGET: u8 = 5;
pub const REQUEST_SIZE: usize = 104;
pub const MAX_RESPONSE: usize = 16384;
pub struct Request(pub [u8; REQUEST_SIZE]);
impl Drop for Request {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            unsafe {
                core::ptr::write_volatile(byte, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}
impl Request {
    pub fn new(command: u8, ssid: &[u8], password: &[u8]) -> Result<Self, &'static str> {
        if !(STATUS..=FORGET).contains(&command)
            || ssid.len() > 32
            || password.len() > 63
            || command != CONNECT && (!ssid.is_empty() || !password.is_empty())
            || command == CONNECT && ssid.is_empty()
            || !password.is_empty() && password.len() < 8
        {
            return Err("Invalid Wi-Fi request");
        }
        let mut out = Self([0; REQUEST_SIZE]);
        out.0[..4].copy_from_slice(b"SWF1");
        out.0[4] = command;
        out.0[5] = ssid.len() as u8;
        out.0[6] = password.len() as u8;
        out.0[8..8 + ssid.len()].copy_from_slice(ssid);
        out.0[40..40 + password.len()].copy_from_slice(password);
        Ok(out)
    }
    pub fn validate(&self) -> bool {
        let n = self.0[5] as usize;
        let p = self.0[6] as usize;
        self.0[..4] == *b"SWF1"
            && self.0[7] == 0
            && n <= 32
            && p <= 63
            && self.0[8 + n..40].iter().all(|v| *v == 0)
            && self.0[40 + p..].iter().all(|v| *v == 0)
            && Self::new(self.0[4], self.ssid(), self.password()).is_ok()
    }
    pub fn ssid(&self) -> &[u8] {
        &self.0[8..8 + (self.0[5] as usize).min(32)]
    }
    pub fn password(&self) -> &[u8] {
        &self.0[40..40 + (self.0[6] as usize).min(63)]
    }
}
pub fn call(command: u8, ssid: &[u8], password: &[u8]) -> Result<String, &'static str> {
    let request = Request::new(command, ssid, password)?;
    let socket = Socket::new().map_err(|_| "Wi-Fi service unavailable")?;
    socket
        .connect(SOCKET)
        .map_err(|_| "Wi-Fi service unavailable")?;
    let stream = socket
        .as_stream()
        .map_err(|_| "Wi-Fi service unavailable")?;
    stream
        .write_all(&request.0)
        .map_err(|_| "Wi-Fi request failed")?;
    drop(request);
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .map_err(|_| "Wi-Fi response failed")?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_RESPONSE {
        return Err("Invalid Wi-Fi response");
    }
    let mut bytes = vec![0; length];
    stream
        .read_exact(&mut bytes)
        .map_err(|_| "Wi-Fi response failed")?;
    let text = String::from_utf8(bytes).map_err(|_| "Invalid Wi-Fi response")?;
    if text.starts_with("ERROR ") {
        if text.trim_end() == "ERROR Disconnect before scanning" {
            return Err("Disconnect before scanning");
        }
        return Err("Wi-Fi request rejected");
    }
    Ok(text)
}
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::new();
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || text.len() > 128 {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let digit = |v: u8| match v {
                b'0'..=b'9' => Some(v - b'0'),
                b'a'..=b'f' => Some(v - b'a' + 10),
                _ => None,
            };
            Some(digit(p[0])? * 16 + digit(p[1])?)
        })
        .collect()
}
#[derive(Clone, Debug)]
pub struct Network {
    pub ssid: Vec<u8>,
    pub akm: u32,
    pub signal: i32,
    pub frequency: u32,
}
pub fn networks(text: &str) -> Vec<Network> {
    text.lines()
        .filter_map(|line| {
            let mut w = line.split_whitespace();
            if w.next()? != "AP" {
                return None;
            }
            let ssid = unhex(w.next()?)?;
            if ssid.is_empty() || ssid.len() > 32 {
                return None;
            }
            Some(Network {
                ssid,
                akm: w.next()?.parse().ok()?,
                signal: w.next()?.parse().ok()?,
                frequency: w.next()?.parse().ok()?,
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_commands_lengths_and_secret_tails_are_rejected() {
        let mut r = Request::new(CONNECT, b"raw\0ssid", b"password").unwrap();
        assert!(r.validate());
        r.0[6] = 64;
        assert!(!r.validate());
        r.0[6] = 8;
        r.0[103] = 1;
        assert!(!r.validate());
        assert!(Request::new(STATUS, b"SSID", b"password").is_err());
        assert!(Request::new(CONNECT, b"", b"password").is_err());
        assert!(Request::new(CONNECT, b"SSID", b"short").is_err());
    }
    #[test]
    fn scan_response_preserves_raw_ssids_and_ignores_invalid_records() {
        let records = networks("OK\nAP ff0001 2 -5000 5180\nAP xx 2 0 2412\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].ssid, vec![255, 0, 1]);
        assert_eq!(unhex(&hex(&[255, 0, 1])), Some(vec![255, 0, 1]));
    }
}
