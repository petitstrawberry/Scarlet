//! Explicit, manually invoked storage checks for real block devices/files.
#![no_std]
#![no_main]
extern crate scarlet_std as std;

use scarlet_os::time::monotonic_time_ns;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    print, println, vec,
};

fn elapsed_ms(started: u64) -> u64 {
    monotonic_time_ns().saturating_sub(started) / 1_000_000
}

fn read_exact(file: &mut File, data: &mut [u8]) -> Result<(), &'static str> {
    let mut offset = 0;
    while offset < data.len() {
        let n = file.read(&mut data[offset..]).map_err(|_| "read failed")?;
        if n == 0 {
            return Err("unexpected EOF");
        }
        offset += n;
    }
    Ok(())
}

fn hash(path: &str, limit: Option<u64>) -> Result<(), &'static str> {
    let mut file = File::open(path).map_err(|_| "cannot open input")?;
    let mut buffer = vec![0; 64 * 1024];
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let started = monotonic_time_ns();
    loop {
        let length = limit
            .map(|limit| (limit - bytes).min(buffer.len() as u64) as usize)
            .unwrap_or(buffer.len());
        if length == 0 {
            break;
        }
        let n = file
            .read(&mut buffer[..length])
            .map_err(|_| "hash read failed")?;
        if n == 0 {
            if limit.is_some() {
                return Err("unexpected EOF before requested byte count");
            }
            break;
        }
        bytes += n as u64;
        hasher.update(&buffer[..n]);
    }
    print!("sha256=");
    for b in hasher.finalize() {
        print!("{:02x}", b);
    }
    println!(
        " bytes={} elapsed_ms={} path={}",
        bytes,
        elapsed_ms(started),
        path
    );
    Ok(())
}

fn inspect(path: &str) -> Result<(), &'static str> {
    let mut file = File::open(path).map_err(|_| "cannot open disk")?;
    let mut mbr = [0u8; 512];
    read_exact(&mut file, &mut mbr)?;
    print!("mbr-sha256=");
    for b in Sha256::digest(mbr) {
        print!("{:02x}", b);
    }
    println!();
    if mbr[510..] != [0x55, 0xaa] {
        return Err("no MBR signature");
    }
    for (index, entry) in mbr[446..510].chunks_exact(16).enumerate() {
        let first = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        let count = u32::from_le_bytes(entry[12..16].try_into().unwrap());
        println!(
            "partition={} type={:#04x} first={} sectors={}",
            index + 1,
            entry[4],
            first,
            count
        );
    }
    Ok(())
}

fn roundtrip(path: &str, mib: u64) -> Result<(), &'static str> {
    // Never open an existing file or a raw device for a destructive check.
    if !path.starts_with('/') || path.starts_with("/dev/") || !(1..=256).contains(&mib) {
        return Err("roundtrip needs a new regular file and 1..256 MiB");
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "cannot create new check file (existing files are preserved)")?;
    let mut block = vec![0u8; 64 * 1024];
    let mut expected = Sha256::new();
    let started = monotonic_time_ns();
    for index in 0..mib * 16 {
        for (word, chunk) in block.chunks_exact_mut(8).enumerate() {
            let value = (index * 8192 + word as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        file.write_all(&block).map_err(|_| "check write failed")?;
        expected.update(&block);
    }
    drop(file);
    let write_ms = elapsed_ms(started);
    let mut file = File::open(path).map_err(|_| "cannot reopen check file")?;
    let mut actual = Sha256::new();
    for _ in 0..mib * 16 {
        read_exact(&mut file, &mut block)?;
        actual.update(&block);
    }
    let mut end = [0];
    if file.read(&mut end).map_err(|_| "EOF read failed")? != 0 {
        return Err("unexpected file length");
    }
    let expected = expected.finalize();
    if expected != actual.finalize() {
        return Err("readback hash mismatch");
    }
    print!(
        "PASS roundtrip bytes={} write_ms={} total_ms={} sha256=",
        mib * 1024 * 1024,
        write_ms,
        elapsed_ms(started)
    );
    for b in expected {
        print!("{:02x}", b);
    }
    println!(" path={}", path);
    Ok(())
}

fn run() -> Result<(), &'static str> {
    let args = std::env::args_vec();
    match args.get(1).map(|s| s.as_str()) {
        Some("inspect") if args.len() == 3 => inspect(&args[2]),
        Some("hash") if args.len() == 3 => hash(&args[2], None),
        Some("hash-prefix") if args.len() == 4 => {
            let mib = args[3].parse::<u64>().map_err(|_| "bad MiB count")?;
            if !(1..=256).contains(&mib) {
                return Err("hash-prefix requires 1..256 MiB");
            }
            hash(&args[2], Some(mib * 1024 * 1024))
        }
        Some("roundtrip") if args.len() == 4 => {
            roundtrip(&args[2], args[3].parse().map_err(|_| "bad MiB count")?)
        }
        Some("mount") if args.len() == 5 => {
            fs::create_directory(&args[3])
                .or_else(|_| fs::list_directory(&args[3]).map(|_| ()))
                .map_err(|_| "cannot create mount point")?;
            let options = std::format!("device={}", args[2]);
            fs::mount(&args[2], &args[3], &args[4], 0, Some(&options))
                .map_err(|_| "mount failed")?;
            println!("mounted {} on {} ({})", args[2], args[3], args[4]);
            Ok(())
        }
        Some("unmount") if args.len() == 3 => {
            fs::unmount(&args[2], 0).map_err(|_| "unmount failed")
        }
        _ => Err(
            "usage: storage-check inspect DEVICE | hash FILE | hash-prefix FILE MiB | roundtrip NEW_FILE MiB | mount DEVICE DIR FSTYPE | unmount DIR",
        ),
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    match run() {
        Ok(()) => 0,
        Err(error) => {
            println!("storage-check: {}", error);
            1
        }
    }
}
