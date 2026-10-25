use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
struct Fixture {
    path: String,
    server: Server,
    id: u32,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "scarlet-sftp-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let path = path.to_str().unwrap().to_owned();
        let mut server = Server::new(&path);
        server.feed(&[0, 0, 0, 5, 1, 0, 0, 0, 3]).unwrap();
        assert_eq!(server.next_response().unwrap().unwrap()[4], 2);
        Self {
            path,
            server,
            id: 0,
        }
    }
    fn packet(&mut self, kind: u8, payload: Vec<u8>) -> Vec<u8> {
        self.id += 1;
        let mut body = response(kind, self.id);
        body.extend(payload);
        let mut framed = Vec::new();
        word(&mut framed, body.len() as u32);
        framed.extend(body);
        framed
    }
    fn call(&mut self, kind: u8, payload: Vec<u8>) -> Vec<u8> {
        let framed = self.packet(kind, payload);
        self.server.feed(&framed).unwrap();
        let out = self.server.next_response().unwrap().unwrap();
        assert_eq!(u32::from_be_bytes(out[5..9].try_into().unwrap()), self.id);
        out
    }
    fn open(&mut self, path: &str, flags: u32) -> Vec<u8> {
        let mut p = Vec::new();
        string(&mut p, path.as_bytes());
        word(&mut p, flags);
        word(&mut p, 0);
        let out = self.call(3, p);
        assert_eq!(out[4], 102, "{out:?}");
        out[9..].to_vec()
    }
    fn close(&mut self, h: Vec<u8>) {
        assert_status(&self.call(4, h), 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
fn path(value: &str) -> Vec<u8> {
    let mut p = Vec::new();
    string(&mut p, value.as_bytes());
    p
}
fn assert_status(out: &[u8], code: u32) {
    assert_eq!(out[4], 101);
    assert_eq!(
        u32::from_be_bytes(out[9..13].try_into().unwrap()),
        code,
        "{out:?}"
    );
}
fn write_at(f: &mut Fixture, h: &[u8], offset: u64, data: &[u8]) {
    let mut p = h.to_vec();
    p.extend(offset.to_be_bytes());
    string(&mut p, data);
    assert_status(&f.call(6, p), 0);
}
fn read_at(f: &mut Fixture, h: &[u8], offset: u64, n: u32) -> Vec<u8> {
    let mut p = h.to_vec();
    p.extend(offset.to_be_bytes());
    word(&mut p, n);
    f.call(5, p)
}

#[test]
fn fragmented_and_pipelined_binary_upload_random_read_and_eof() {
    let mut f = Fixture::new();
    let h = f.open("日本語 file.bin", 2 | 8 | 16);
    let bytes: Vec<u8> = (0..70001).map(|n| (n % 256) as u8).collect();
    let mut p = h.clone();
    p.extend(0u64.to_be_bytes());
    string(&mut p, &bytes);
    let packet = f.packet(6, p);
    for chunk in packet[..packet.len() - 1].chunks(17) {
        f.server.feed(chunk).unwrap();
        assert!(f.server.next_response().unwrap().is_none());
    }
    f.server.feed(&packet[packet.len() - 1..]).unwrap();
    assert_status(&f.server.next_response().unwrap().unwrap(), 0);
    f.close(h);
    let h = f.open("日本語 file.bin", 1);
    let out = read_at(&mut f, &h, 32000, 32768);
    assert_eq!(out[4], 103);
    assert_eq!(&out[13..], &bytes[32000..64768]);
    assert_status(&read_at(&mut f, &h, bytes.len() as u64, 4), 1);
    let mut a = path(".");
    let pa = f.packet(16, core::mem::take(&mut a));
    let pb = f.packet(17, path("日本語 file.bin"));
    f.server.feed(&[pa, pb].concat()).unwrap();
    assert_eq!(f.server.next_response().unwrap().unwrap()[4], 104);
    assert_eq!(f.server.next_response().unwrap().unwrap()[4], 105);
}
#[test]
fn exclusive_open_stale_and_readonly_handles_do_not_change_files() {
    let mut f = Fixture::new();
    let h = f.open("keep", 2 | 8 | 32);
    write_at(&mut f, &h, 0, b"untouched");
    f.close(h.clone());
    let mut p = path("keep");
    word(&mut p, 2 | 8 | 32 | 16);
    word(&mut p, 0);
    assert_status(&f.call(3, p), 4);
    assert_eq!(fs::read(format!("{}/keep", f.path)).unwrap(), b"untouched");
    assert_status(&f.call(4, h), 4);
    let h = f.open("keep", 1);
    let mut p = h;
    p.extend(0u64.to_be_bytes());
    string(&mut p, b"destroy");
    assert_status(&f.call(6, p), 3);
}
#[test]
fn handle_admission_precedes_truncate_and_reuses_no_closed_ids() {
    let mut f = Fixture::new();
    fs::write(format!("{}/keep", f.path), b"keep").unwrap();
    let mut handles = Vec::new();
    for _ in 0..MAX_HANDLES {
        handles.push(f.open("keep", 1));
    }
    let mut p = path("keep");
    word(&mut p, 2 | 16);
    word(&mut p, 0);
    assert_status(&f.call(3, p), 4);
    assert_eq!(fs::read(format!("{}/keep", f.path)).unwrap(), b"keep");
    let old = handles.pop().unwrap();
    f.close(old.clone());
    let new = f.open("keep", 1);
    assert_ne!(old, new);
    assert_status(&f.call(8, old), 4);
}
#[test]
fn rename_never_replaces_without_extension_and_open_file_identity_survives() {
    let mut f = Fixture::new();
    let h = f.open("a", 1 | 2 | 8);
    write_at(&mut f, &h, 0, b"a");
    fs::write(format!("{}/b", f.path), b"b").unwrap();
    let mut p = path("a");
    p.extend(path("b"));
    assert_status(&f.call(18, p.clone()), 4);
    assert_eq!(fs::read(format!("{}/b", f.path)).unwrap(), b"b");
    let mut extension = path("posix-rename@openssh.com");
    extension.extend(p);
    assert_status(&f.call(200, extension), 0);
    write_at(&mut f, &h, 0, b"c");
    assert_eq!(fs::read(format!("{}/b", f.path)).unwrap(), b"c");
    assert_eq!(f.call(8, h)[4], 105);
}
#[test]
fn directories_are_paged_and_symlinks_and_realpath_preserve_kernel_resolution() {
    let mut f = Fixture::new();
    let mut p = path("dir");
    word(&mut p, 0);
    assert_status(&f.call(14, p), 0);
    for i in 0..70 {
        fs::write(format!("{}/dir/{i}", f.path), b"x").unwrap();
    }
    let out = f.call(11, path("dir"));
    let h = out[9..].to_vec();
    let mut total = 0;
    loop {
        let out = f.call(12, h.clone());
        if out[4] == 101 {
            assert_status(&out, 1);
            break;
        }
        let count = u32::from_be_bytes(out[9..13].try_into().unwrap());
        assert!(count <= 32);
        total += count;
    }
    assert_eq!(total, 70);
    f.close(h);
    let mut p = path("dir");
    p.extend(path("link"));
    assert_status(&f.call(20, p), 0);
    let l = f.call(7, path("link"));
    let s = f.call(17, path("link"));
    assert_eq!(
        u32::from_be_bytes(l[21..25].try_into().unwrap()) & 0o170000,
        0o120000
    );
    assert_eq!(
        u32::from_be_bytes(s[21..25].try_into().unwrap()) & 0o170000,
        0o040000
    );
    assert_eq!(f.call(19, path("link"))[4], 104);
    assert_eq!(f.call(16, path("link/.."))[4], 104);
    let mut p = path("new");
    word(&mut p, 0x8000_0000);
    word(&mut p, 1);
    string(&mut p, b"unknown");
    string(&mut p, b"x");
    assert_status(&f.call(14, p), 8);
    assert!(!std::path::Path::new(&format!("{}/new", f.path)).exists());
}
#[test]
fn malformed_lengths_attributes_and_large_reads_are_bounded() {
    let mut server = Server::new("/root");
    server
        .feed(&((MAX_PACKET as u32 + 1).to_be_bytes()))
        .unwrap();
    assert!(server.next_response().is_err());
    let mut f = Fixture::new();
    let h = f.open("small", 1 | 2 | 8);
    write_at(&mut f, &h, 0, b"hello");
    let out = read_at(&mut f, &h, 0, u32::MAX);
    assert_eq!(&out[13..], b"hello");
    let mut p = path("small");
    word(&mut p, 1 | 2);
    p.extend(0u64.to_be_bytes());
    word(&mut p, 1);
    word(&mut p, 1);
    assert_status(&f.call(9, p), 8);
    assert_eq!(fs::read(format!("{}/small", f.path)).unwrap(), b"hello");
    let mut p = path("bad\0path");
    word(&mut p, 2 | 8 | 16);
    word(&mut p, 0);
    assert_status(&f.call(3, p), 5);
    let mut p = path("small");
    word(&mut p, 2 | 16);
    word(&mut p, 0);
    p.push(1);
    assert_status(&f.call(3, p), 5);
    assert_eq!(fs::read(format!("{}/small", f.path)).unwrap(), b"hello");
}
