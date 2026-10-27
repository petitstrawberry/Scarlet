#![no_std]
#![no_main]
extern crate scarlet_std as std;
use scarlet_os::wireless_service as api;
use std::{env, fs::File, println, thread, time::Duration};
use zeroize::Zeroizing;
#[unsafe(no_mangle)]
fn main() -> i32 {
    let args = env::args().collect::<std::vec::Vec<_>>();
    let op = match args.get(1).map(|s| s.as_str()) {
        Some("status") => api::STATUS,
        Some("scan") => api::SCAN,
        Some("connect") => api::CONNECT,
        Some("disconnect") => api::DISCONNECT,
        Some("forget") => api::FORGET,
        _ => {
            println!(
                "usage: wifictl status|scan|disconnect|forget|connect SSID [--password-file PATH]"
            );
            return 2;
        }
    };
    let mut password = Zeroizing::new([0u8; 66]);
    let mut length = 0;
    if op == api::CONNECT && args.len() > 3 {
        if args.len() != 5 || args[3] != "--password-file" {
            return 2;
        }
        let Ok(mut f) = File::open(&args[4]) else {
            println!("Cannot read password file");
            return 2;
        };
        loop {
            let Ok(n) = f.read(&mut password[length..]) else {
                return 2;
            };
            if n == 0 {
                break;
            }
            length += n;
            if length == password.len() {
                println!("Password file too long");
                return 2;
            }
        }
        while length > 0 && matches!(password[length - 1], b'\r' | b'\n') {
            length -= 1;
        }
    }
    let ssid = if op == api::CONNECT {
        match args.get(2) {
            Some(s) => s.as_bytes(),
            None => return 2,
        }
    } else {
        &[]
    };
    match api::call(op, ssid, &password[..length]) {
        Ok(text) => {
            if op == api::STATUS {
                println!("{}", text);
                return 0;
            }
            println!("Wi-Fi request accepted");
            for _ in 0..900 {
                thread::sleep(Duration::from_millis(100));
                if let Ok(text) = api::call(api::STATUS, &[], &[]) {
                    let ready = if op == api::CONNECT {
                        text.contains("STATE Connected")
                            && text.lines().any(|l| {
                                l.starts_with("LINK wlan0 ")
                                    && !l.starts_with("LINK wlan0 0.0.0.0 ")
                            })
                    } else if op == api::SCAN {
                        text.contains("STATE Choose a network")
                    } else {
                        text.contains("STATE Disconnected")
                    };
                    if ready {
                        println!("{}", text);
                        return 0;
                    }
                    if text.contains("STATE Password required") {
                        println!("Password required");
                        return 1;
                    }
                    if op == api::SCAN
                        && (text.contains("STATE Scan failed")
                            || text.contains("STATE Scan unavailable")
                            || text.contains("STATE Wi-Fi unavailable")
                            || text.contains("STATE Wi-Fi busy")
                            || text.contains("STATE Scan timed out"))
                    {
                        println!("{}", text);
                        return 1;
                    }
                }
            }
            println!("Wi-Fi operation timed out");
            1
        }
        Err(e) => {
            println!("{}", e);
            1
        }
    }
}
