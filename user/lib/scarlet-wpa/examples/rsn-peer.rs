//! Host-only protocol test endpoint. Secret key bytes are never printed.
use scarlet_wpa::{Adapter, Engine, Error, WPA2_PSK_CCMP_IE};
use sha2::{Digest, Sha256};
use std::{
    io::{self, BufRead, Read, Write},
    time::Instant,
};
struct Driver {
    events: Vec<String>,
    live: usize,
    installs: usize,
    aborts: usize,
    entropy: bool,
    key_failure: bool,
    start: Instant,
    advance: u64,
    authorized: bool,
    fingerprints: Vec<[u8; 32]>,
}
impl Adapter for Driver {
    fn random(&mut self, out: &mut [u8]) -> Result<(), Error> {
        if !self.entropy {
            return Err(Error::EntropyUnavailable);
        }
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(out))
            .map_err(|_| Error::EntropyUnavailable)
    }
    fn now_ns(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64 + self.advance
    }
    fn send(&mut self, _: [u8; 6], pdu: &[u8]) -> Result<(), Error> {
        self.events.push(format!("TX {}", hex(pdu)));
        Ok(())
    }
    fn key(
        &mut self,
        kind: u32,
        _: [u8; 6],
        index: u32,
        _: &[u8],
        key: &[u8],
    ) -> Result<(), Error> {
        if self.key_failure {
            return Err(Error::Driver);
        }
        if kind == 0 {
            self.live = self.live.saturating_sub(1);
            return Ok(());
        }
        let fingerprint: [u8; 32] = Sha256::digest(key).into();
        if self.fingerprints.contains(&fingerprint) {
            return Err(Error::Driver);
        }
        self.fingerprints.push(fingerprint);
        if kind != 2 || !self.authorized {
            self.live += 1;
        }
        self.installs += 1;
        self.events
            .push(format!("KEY {kind} {index} {}", hex(&Sha256::digest(key))));
        Ok(())
    }
    fn set_authorized(&mut self, value: bool) -> Result<(), Error> {
        self.authorized = value;
        self.events.push(format!("PORT {}", value as u8));
        Ok(())
    }
    fn require_protected_group(&self) -> bool {
        true
    }
    fn abort(&mut self) {
        self.live = 0;
        self.aborts += 1;
    }
}
fn hex(buf: &[u8]) -> String {
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(input: &str) -> Vec<u8> {
    (0..input.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&input[i..i + 2], 16).unwrap())
        .collect()
}
fn main() {
    let pass = std::env::var("SCARLET_WPA_TEST_PASSWORD").unwrap_or("correct password".into());
    let d = Driver {
        events: Vec::new(),
        live: 0,
        installs: 0,
        aborts: 0,
        entropy: true,
        key_failure: false,
        start: Instant::now(),
        advance: 0,
        authorized: false,
        fingerprints: Vec::new(),
    };
    let peer = [2, 0, 0, 0, 0, 1];
    let own = [0x52, 0x54, 0, 0x12, 0x34, 0];
    let mut ap_ies = WPA2_PSK_CCMP_IE.to_vec();
    if let Ok(extension) = std::env::var("SCARLET_WPA_TEST_RSNXE") {
        ap_ies.extend_from_slice(&unhex(&extension));
    }
    let mut e = Engine::new(d, own, peer, b"ScarletMock", &pass, &ap_ies, 5_000_000_000).unwrap();
    println!("READY");
    io::stdout().flush().unwrap();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let mut words = line.split_whitespace();
        let command = words.next().unwrap_or("");
        let result = match command {
            "RXP" => e
                .receive_protected(peer, &unhex(words.next().unwrap()), true)
                .map(|_| ()),
            "RX" => e.receive(peer, &unhex(words.next().unwrap())).map(|_| ()),
            "TICK" => {
                e.adapter_mut().advance = words.next().unwrap().parse().unwrap();
                e.tick()
            }
            "NO_ENTROPY" => {
                e.adapter_mut().entropy = false;
                Ok(())
            }
            "FAIL_KEY" => {
                e.adapter_mut().key_failure = true;
                Ok(())
            }
            "STATUS" => Ok(()),
            "QUIT" => break,
            _ => panic!("invalid test command"),
        };
        let authorized = e.authorized();
        let d = e.adapter_mut();
        for event in d.events.drain(..) {
            println!("{event}");
        }
        println!(
            "RESULT {:?} AUTH {} LIVE {} INSTALLS {} ABORTS {}",
            result, authorized as u8, d.live, d.installs, d.aborts
        );
        io::stdout().flush().unwrap();
    }
}
