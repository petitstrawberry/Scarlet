#![no_std]
#![no_main]
extern crate alloc;
extern crate scarlet_std as std;
mod events;
use alloc::collections::VecDeque;
use scarlet_os::{socket::Socket, time::monotonic_time_ns, wireless::*, wireless_service as api};
use scarlet_wpa::{Engine, derive_pmk, native::NativeAdapter, profile::Profile};
use std::{
    format,
    fs::File,
    io::Write,
    println,
    string::String,
    sync::{Arc, Mutex},
    thread,
    vec::Vec,
};
use zeroize::{Zeroize, Zeroizing};
const PROFILE: &str = "/etc/wireless.conf";
struct Shared {
    queue: VecDeque<api::Request>,
    status: String,
    aps: Vec<Bss>,
    ssid: Vec<u8>,
    can_scan: bool,
}
fn read_profile() -> Option<Profile> {
    let mut f = File::open(PROFILE).ok()?;
    let mut bytes = Zeroizing::new([0u8; 4097]);
    let mut used = 0;
    loop {
        let n = f.read(&mut bytes[used..]).ok()?;
        if n == 0 {
            break;
        }
        used += n;
        if used > 4096 {
            return None;
        }
    }
    Profile::parse(core::str::from_utf8(&bytes[..used]).ok()?).ok()
}
fn save_profile(p: &Profile) -> Result<(), &'static str> {
    let psk = Zeroizing::new(api::hex(&p.pmk[..]));
    let mut text = Zeroizing::new(format!(
        "network={{\nssid={}\npsk={}\n}}\n",
        api::hex(&p.ssid),
        psk.as_str()
    ));
    let staged = "/etc/wireless.conf.tmp";
    let result = (|| {
        let mut f = File::create(staged).map_err(|_| "Could not save network")?;
        f.write_all(text.as_bytes())
            .map_err(|_| "Could not save network")?;
        f.flush().map_err(|_| "Could not save network")?;
        drop(f);
        std::fs::rename(staged, PROFILE).map_err(|_| "Could not save network")
    })();
    text.zeroize();
    if result.is_err() {
        let _ = std::fs::remove_file(staged);
    }
    result
}
fn status(shared: &Arc<Mutex<Shared>>, message: &str) {
    shared.lock().status = String::from(message);
}
fn snapshot(shared: &Arc<Mutex<Shared>>) -> String {
    let s = shared.lock();
    let mut text = format!(
        "OK\nSTATE {}\nSSID {}\nCAN_SCAN {}\n",
        s.status,
        api::hex(&s.ssid),
        u8::from(s.can_scan)
    );
    for b in &s.aps {
        text.push_str(&format!(
            "AP {} {} {} {}\n",
            api::hex(&b.ssid[..b.ssid_len as usize]),
            b.akm,
            b.signal_mbm,
            b.frequency_mhz
        ));
    }
    drop(s);
    for c in scarlet_os::network::list_interface_configs().unwrap_or_default() {
        let ip = c.ip_address;
        let gw = c.gateway;
        text.push_str(&format!(
            "LINK {} {}.{}.{}.{} {}.{}.{}.{}\n",
            c.interface_name().unwrap_or("?"),
            ip[0],
            ip[1],
            ip[2],
            ip[3],
            gw[0],
            gw[1],
            gw[2],
            gw[3]
        ));
    }
    text
}
fn ipc(shared: Arc<Mutex<Shared>>, events: Arc<events::Events>) {
    let listener = match Socket::new() {
        Ok(s) => s,
        Err(_) => return,
    };
    if listener.bind(api::SOCKET).is_err() || listener.listen(8).is_err() {
        println!("wirelessd: control socket unavailable");
        return;
    }
    loop {
        let Ok(client) = listener.accept() else {
            continue;
        };
        let Ok(stream) = client.as_stream() else {
            continue;
        };
        let mut req = api::Request([0; api::REQUEST_SIZE]);
        if stream.read_exact(&mut req.0).is_err() {
            continue;
        }
        let text = if !req.validate() {
            String::from("ERROR Invalid request\n")
        } else if req.0[4] == api::STATUS {
            snapshot(&shared)
        } else {
            let mut s = shared.lock();
            if req.0[4] == api::SCAN && !s.can_scan {
                String::from("ERROR Disconnect before scanning\n")
            } else if s.queue.len() >= 8 {
                String::from("ERROR Busy\n")
            } else {
                s.status = String::from(match req.0[4] {
                    api::SCAN => "Scanning",
                    api::CONNECT => "Connecting",
                    _ => "Disconnecting",
                });
                s.queue.push_back(req);
                s.can_scan = false;
                events.notify();
                String::from("OK\n")
            }
        };
        let _ = stream.write_all(&(text.len() as u32).to_le_bytes());
        let _ = stream.write_all(text.as_bytes());
    }
}
fn scan(session: &mut WirelessSession) -> Result<Vec<Bss>, &'static str> {
    let id = session.scan(25000).map_err(|_| "Scan unavailable")?;
    let deadline = monotonic_time_ns().saturating_add(26_000_000_000);
    let info = loop {
        while let Some(event) = session.next_event().map_err(|_| "Scan event failed")? {
            match event.kind {
                EVENT_OVERFLOW => {
                    session
                        .resync()
                        .map_err(|_| "Scan resynchronization failed")?;
                }
                EVENT_RESET | EVENT_REMOVED => return Err("Wi-Fi unavailable"),
                _ => {}
            }
        }
        let info = session.request(id).map_err(|_| "Scan failed")?;
        if info.result != ACCEPTED {
            break info;
        }
        if monotonic_time_ns() >= deadline {
            let _ = session.cancel(id);
            return Err("Scan timed out");
        }
        let mut handles = [scarlet_os::poll::PollHandle::new(
            session.handle().as_raw() as u32,
            scarlet_os::poll::POLLIN,
        )];
        scarlet_os::poll::poll(
            &mut handles,
            deadline.saturating_sub(monotonic_time_ns()) as i64,
        )
        .map_err(|_| "Scan wait failed")?;
    };
    let _ = session.acknowledge(id);
    if info.result != OK {
        return Err("Scan failed");
    }
    let mut entries = Vec::new();
    let mut cursor = 0;
    loop {
        let (entry, next) = session
            .scan_result(info.object_id, cursor)
            .map_err(|_| "Scan results unavailable")?;
        let Some(entry) = entry else { break };
        if entry.ssid_len > 0
            && !entries
                .iter()
                .any(|b: &Bss| b.ssid_len == entry.ssid_len && b.ssid == entry.ssid)
        {
            entries.push(entry);
        }
        cursor = next;
        if entries.len() >= 64 {
            break;
        }
    }
    let _ = session.release_scan(info.object_id);
    Ok(entries)
}
fn connect(profile: &Profile) -> Result<Engine<NativeAdapter>, scarlet_wpa::Error> {
    let adapter = NativeAdapter::connect("/dev/wifi0", &profile.ssid, 45000)?;
    let own = adapter.own;
    let peer = adapter.peer;
    let ap = adapter.ap_rsn_ie().to_vec();
    Engine::from_pmk(
        adapter,
        own,
        peer,
        &profile.ssid,
        &profile.pmk,
        &ap,
        30_000_000_000,
    )
}
#[unsafe(no_mangle)]
fn main() -> i32 {
    let shared = Arc::new(Mutex::new(Shared {
        queue: VecDeque::new(),
        status: String::from("Waiting for Wi-Fi"),
        aps: Vec::new(),
        ssid: Vec::new(),
        can_scan: false,
    }));
    let ipc_shared = shared.clone();
    let events = Arc::new(events::Events::new());
    let ipc_events = events.clone();
    thread::spawn(move || ipc(ipc_shared, ipc_events));
    let mut profile = read_profile();
    let mut enabled = profile.is_some();
    let mut engine: Option<Engine<NativeAdapter>> = None;
    let mut next_retry = 0;
    let mut backoff = 1u64;
    let mut save_pending = false;
    println!("wirelessd: started");
    loop {
        events.drain();
        events.watch_radio();
        let request = shared.lock().queue.pop_front();
        if let Some(request) = request {
            match request.0[4] {
                api::CONNECT => {
                    let new_profile = if request.password().is_empty() {
                        read_profile().filter(|p| p.ssid == request.ssid())
                    } else {
                        core::str::from_utf8(request.password())
                            .ok()
                            .and_then(|pass| derive_pmk(pass, request.ssid()).ok())
                            .map(|pmk| Profile {
                                ssid: request.ssid().to_vec(),
                                pmk: Zeroizing::new(pmk),
                            })
                    };
                    if let Some(p) = new_profile {
                        engine = None;
                        shared.lock().ssid = p.ssid.clone();
                        profile = Some(p);
                        enabled = true;
                        next_retry = 0;
                        backoff = 1;
                        save_pending = !request.password().is_empty();
                        status(&shared, "Connecting");
                    } else {
                        status(&shared, "Password required for this network");
                    }
                }
                api::DISCONNECT | api::FORGET => {
                    engine = None;
                    enabled = false;
                    save_pending = false;
                    shared.lock().ssid.clear();
                    if request.0[4] == api::FORGET {
                        let _ = std::fs::remove_file(PROFILE);
                        profile = None;
                    }
                    status(&shared, "Disconnected");
                }
                api::SCAN => {
                    status(&shared, "Scanning");
                    let result = if let Some(e) = engine.as_mut() {
                        scan(e.adapter_mut().session_mut())
                    } else {
                        WirelessSession::open("/dev/wifi0")
                            .map_err(|_| "Wi-Fi unavailable")
                            .and_then(|mut session| {
                                session.claim_controller().map_err(|_| "Wi-Fi busy")?;
                                session.subscribe().map_err(|_| "Wi-Fi unavailable")?;
                                scan(&mut session)
                            })
                    };
                    match result {
                        Ok(aps) => {
                            shared.lock().aps = aps;
                            status(
                                &shared,
                                if engine.is_some() {
                                    "Connected"
                                } else {
                                    "Choose a network"
                                },
                            );
                        }
                        Err(e) => status(&shared, e),
                    }
                }
                _ => {}
            }
        }
        if enabled && engine.is_none() && monotonic_time_ns() >= next_retry {
            if let Some(p) = &profile {
                shared.lock().ssid = p.ssid.clone();
                status(&shared, "Connecting");
                match connect(p) {
                    Ok(e) => {
                        engine = Some(e);
                        status(&shared, "Authenticating");
                    }
                    Err(_) => {
                        status(&shared, "Connection failed; retrying");
                        next_retry = monotonic_time_ns() + backoff * 1_000_000_000;
                        backoff = (backoff * 2).min(30);
                    }
                }
            }
        }
        if let Some(e) = engine.as_mut() {
            let result = (|| {
                while let Some(frame) = e.adapter_mut().next_eapol()? {
                    e.receive_protected(
                        frame.peer,
                        &frame.payload[..frame.length as usize],
                        frame.flags & scarlet_wpa::EAPOL_PROTECTED != 0,
                    )?;
                }
                e.tick()
            })();
            if result.is_err() {
                engine = None;
                status(&shared, "Authentication failed; retrying");
                next_retry = monotonic_time_ns() + backoff * 1_000_000_000;
                backoff = (backoff * 2).min(30);
            } else if e.authorized() {
                backoff = 1;
                if save_pending {
                    if profile.as_ref().is_some_and(|p| save_profile(p).is_err()) {
                        status(&shared, "Connected; could not save network");
                    } else {
                        status(&shared, "Connected");
                    }
                    save_pending = false;
                } else if shared.lock().status == "Authenticating" {
                    status(&shared, "Connected");
                }
            }
        }
        shared.lock().can_scan = engine.is_none() && !enabled;
        if !shared.lock().queue.is_empty() {
            continue;
        }
        let deadline = if let Some(e) = &engine {
            e.next_deadline_ns()
        } else if enabled && profile.is_some() {
            Some(next_retry)
        } else {
            None
        };
        events.wait(deadline);
    }
}
