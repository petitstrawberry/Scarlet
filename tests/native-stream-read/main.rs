//! Run against the native kernel; exercises short reads through std's real PAL.
use std::{fs, io::{Read, Write}, thread, time::Duration};

fn main() {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(30));
        eprintln!("SCARLET_BOUNDED_STREAM_READ_FAIL timeout");
        std::process::exit(1);
    });
    let path = "/tmp/bounded-stream-read.bin";
    let expected: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    fs::File::create(path).unwrap().write_all(&expected).unwrap();
    let mut file = fs::File::open(path).unwrap();
    // The caller may offer a much larger buffer than a syscall should allocate.
    let mut offered = vec![0; 4 * 1024 * 1024];
    let n = file.read(&mut offered).unwrap();
    assert!(n > 0 && n <= 64 * 1024, "unbounded read: {n}");
    assert_eq!(&offered[..n], &expected[..n]);
    let mut rest = Vec::new();
    file.read_to_end(&mut rest).unwrap();
    assert_eq!(rest, expected[n..], "short reads lost or duplicated bytes");
    assert_eq!(file.read(&mut offered).unwrap(), 0);
    drop(file);
    fs::remove_file(path).unwrap();
    println!("SCARLET_BOUNDED_STREAM_READ_OK");
}
