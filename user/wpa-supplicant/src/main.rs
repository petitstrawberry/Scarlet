#![no_std]
#![no_main]
extern crate scarlet_std as std;
use scarlet_wpa::{Engine, Error, native::NativeAdapter, profile::Profile};
use std::{env, fs::File, println, thread, time::Duration};
use zeroize::Zeroizing;
fn run(path: &str, profile: &Profile) -> Result<(), Error> {
    let adapter = NativeAdapter::connect(path, &profile.ssid, 15000)?;
    let own = adapter.own;
    let peer = adapter.peer;
    let ap = adapter.ap_rsn_ie().to_vec();
    let mut engine = Engine::from_pmk(
        adapter,
        own,
        peer,
        &profile.ssid,
        &profile.pmk,
        &ap,
        5_000_000_000,
    )?;
    let mut reported = false;
    loop {
        if let Some(frame) = engine.adapter_mut().next_eapol()? {
            engine.receive_protected(
                frame.peer,
                &frame.payload[..frame.length as usize],
                frame.flags & scarlet_wpa::EAPOL_PROTECTED != 0,
            )?;
        } else {
            let deadline = engine.next_deadline_ns();
            engine.adapter_mut().wait_until(deadline)?;
        }
        engine.tick()?;
        if engine.authorized() && !reported {
            println!("wpa-supplicant: authenticated");
            reported = true;
        }
    }
}
#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args().collect::<std::vec::Vec<_>>();
    if args.len() != 3 {
        println!("usage: wpa-supplicant /dev/wifiN /etc/wpa_supplicant.conf");
        return 2;
    }
    let mut bytes = Zeroizing::new([0u8; 4097]);
    let mut used = 0;
    let mut file = match File::open(&args[2]) {
        Ok(f) => f,
        Err(_) => {
            println!("wpa-supplicant: cannot open configuration");
            return 2;
        }
    };
    loop {
        match file.read(&mut bytes[used..]) {
            Ok(0) => break,
            Ok(n) => {
                used += n;
                if used > 4096 {
                    println!("wpa-supplicant: configuration too large");
                    return 2;
                }
            }
            Err(_) => {
                println!("wpa-supplicant: cannot read configuration");
                return 2;
            }
        }
    }
    let profile = match core::str::from_utf8(&bytes[..used])
        .ok()
        .and_then(|text| Profile::parse(text).ok())
    {
        Some(p) => p,
        None => {
            println!("wpa-supplicant: invalid WPA2-Personal/CCMP configuration");
            return 2;
        }
    };
    use zeroize::Zeroize;
    bytes.as_mut_slice().zeroize();
    let mut backoff = 1;
    loop {
        if let Err(error) = run(&args[1], &profile) {
            println!("wpa-supplicant: authentication closed ({:?})", error);
        }
        thread::sleep(Duration::from_secs(backoff));
        backoff = (backoff * 2).min(30);
    }
}
