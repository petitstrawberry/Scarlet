//! Execute extracted baseline/candidate metadata and header implementations.
#![allow(dead_code)]
extern crate alloc;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static COUNT: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if COUNT.load(Ordering::Relaxed) {
            REALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
/* BASELINE */
/* CANDIDATE */

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCS.store(0, Ordering::Relaxed);
    REALLOCS.store(0, Ordering::Relaxed);
    COUNT.store(true, Ordering::Relaxed);
    let value = f();
    std::hint::black_box(&value);
    COUNT.store(false, Ordering::Relaxed);
    (
        value,
        ALLOCS.load(Ordering::Relaxed),
        REALLOCS.load(Ordering::Relaxed),
    )
}
macro_rules! metadata {
    ($context:ty) => {{
        let mut ip = <$context>::new();
        ip.set("ip_dst", &[192, 168, 0, 16]);
        ip.set("ip_src", &[192, 168, 0, 35]);
        ip.set("ip_protocol", &[6]);
        ip.set("interface", b"usbnet0");
        let mut eth = ip.clone();
        eth.set("eth_type", &[8, 0]);
        eth.set("interface", b"usbnet0");
        eth.set("ip_src", &[192, 168, 0, 35]);
        eth.set("next_hop", &[192, 168, 0, 16]);
        (ip, eth)
    }};
}
#[test]
fn ordinary_tcp_ip_ethernet_metadata_uses_no_heap() {
    let ((a, b), old_alloc, old_realloc) = counted(|| metadata!(baseline::LayerContext));
    let ((c, d), new_alloc, new_realloc) = counted(|| metadata!(candidate::LayerContext));
    for key in [
        "ip_src",
        "ip_dst",
        "ip_protocol",
        "interface",
        "eth_type",
        "next_hop",
    ] {
        assert_eq!(a.get(key), c.get(key));
        assert_eq!(b.get(key), d.get(key));
    }
    assert!(old_alloc > 0);
    assert_eq!((new_alloc, new_realloc), (0, 0));
    println!(
        "metadata construction/clone/update: allocations {old_alloc}->{new_alloc}; reallocations {old_realloc}->{new_realloc}"
    );
    assert!(std::mem::size_of::<candidate::LayerContext>() <= 640);
}
#[test]
fn arbitrary_keys_values_overflow_updates_and_clone_match_map() {
    let mut a = baseline::LayerContext::new();
    let mut b = candidate::LayerContext::new();
    let keys: Vec<String> = (0..80)
        .map(|i| {
            format!(
                "{}-{i}",
                if i % 3 == 0 {
                    "任意の長いプロトコル名"
                } else {
                    "key"
                }
            )
        })
        .collect();
    let lengths = [0, 1, 8, 32, 33, 128, 4096];
    for n in 0..1200 {
        let k = &keys[(n * 37) % keys.len()];
        let v = vec![(n % 256) as u8; lengths[n % lengths.len()]];
        a.set(k, &v);
        b.set(k, &v);
        for key in &keys {
            assert_eq!(a.get(key), b.get(key));
            assert_eq!(a.contains(key), b.contains(key));
        }
    }
    let mut copy = b.clone();
    copy.set(&keys[0], b"changed");
    assert_eq!(b.get(&keys[0]), a.get(&keys[0]));
    assert_eq!(copy.get(&keys[0]), Some(b"changed".as_slice()));
}
#[test]
fn inline_boundary_promotes_without_duplicate_or_truncated_values() {
    let mut c = candidate::LayerContext::new();
    c.set("", &[]);
    assert!(c.contains(""));
    assert_eq!(c.get(""), Some([].as_slice()));
    let k = "123456789012345678901234";
    c.set(k, &[1; 32]);
    c.set(k, &[2; 33]);
    assert_eq!(c.get(k), Some([2; 33].as_slice()));
    c.set(k, &[]);
    assert_eq!(c.get(k), Some([].as_slice()));
    c.set("other", b"new");
    assert_eq!(c.get("other"), Some(b"new".as_slice()));
    assert!(!c.contains("missing"));
}
#[test]
fn inline_input_is_owned_and_clones_are_independent() {
    let mut k = String::from("arbitrary");
    let mut v = vec![0x77; 16];
    let mut c = candidate::LayerContext::new();
    c.set(&k, &v);
    k.clear();
    v.fill(0);
    drop(k);
    drop(v);
    let mut d = c.clone();
    d.set("arbitrary", b"different");
    assert_eq!(c.get("arbitrary"), Some([0x77; 16].as_slice()));
}
macro_rules! tcp_header {
    ($module:ident) => {
        $module::TcpHeader {
            src_port: 12345,
            dst_port: 443,
            seq_number: 0xff123456,
            ack_number: 0xabcd1234,
            data_offset_flags: 0x5010,
            window_size: 32767,
            checksum: 0xabcd,
            urgent_pointer: 7,
        }
    };
}
#[test]
fn tcp_headers_and_checksums_match_across_odd_boundaries_and_large_payloads() {
    let old = tcp_header!(baseline);
    let new = tcp_header!(candidate);
    assert_eq!(old.to_bytes(), new.to_bytes());
    assert_eq!(old.to_bytes(), new.to_array());
    for size in [0, 1, 2, 3, 20, 21, 512, 1448, 1460, 65535, 131073] {
        let data: Vec<u8> = (0..size).map(|i| ((i * 31 + i / 7) % 256) as u8).collect();
        for option_size in [0, 1, 2, 3, 4, 7, 40] {
            let options: Vec<u8> = (0..option_size).map(|i| (i * 17) as u8).collect();
            assert_eq!(
                old.calculate_checksum_with_options([1, 2, 3, 4], [5, 6, 7, 8], &options, &data),
                new.calculate_checksum_with_options([1, 2, 3, 4], [5, 6, 7, 8], &options, &data),
                "data={size} options={option_size}"
            );
        }
    }
}
#[test]
fn tcp_checksum_has_no_heap_and_does_not_modify_header() {
    let h = tcp_header!(candidate);
    let before = h.to_bytes();
    let (sum, a, r) =
        counted(|| h.calculate_checksum_with_options([1; 4], [2; 4], &[7; 3], &[0x5a; 1461]));
    assert_eq!((a, r), (0, 0));
    assert_eq!(h.to_bytes(), before);
    std::hint::black_box(sum);
    let h = tcp_header!(baseline);
    let (_, a, r) =
        counted(|| h.calculate_checksum_with_options([1; 4], [2; 4], &[7; 3], &[0x5a; 1461]));
    println!("TCP checksum: allocations {a}->0; reallocations {r}->0");
    assert!(a > 0);
}
#[test]
fn ipv4_serialization_and_checksum_match_without_heap() {
    let mut a = baseline::Ipv4Header::new();
    let mut b = candidate::Ipv4Header::new();
    for seed in 0..256u16 {
        a.total_length = seed * 251;
        b.total_length = a.total_length;
        a.checksum = seed;
        b.checksum = seed;
        a.tos = seed as u8;
        b.tos = a.tos;
        a.source_ip = [192, 168, seed as u8, 1];
        b.source_ip = a.source_ip;
        assert_eq!(a.to_bytes(), b.to_array());
        assert_eq!(a.calculate_checksum(), b.calculate_checksum());
    }
    let (_, allocs, reallocs) = counted(|| b.calculate_checksum());
    assert_eq!((allocs, reallocs), (0, 0));
}


mod socket {
    #[derive(Debug, PartialEq)]
    pub enum SocketError { InvalidPacket, Other(String) }
}
mod owned_packet {
    /* OWNED_PACKET */
}

#[test]
fn encapsulation_transfers_one_allocation_and_no_payload_moves() {
    use owned_packet::PacketBuffer;
    for size in [0, 1, 20, 1460, 65501] {
        let payload = vec![0xa5; size];
        let (frame, allocs, reallocs) = counted(|| {
            let mut packet = PacketBuffer::from_slice(&payload, 34).unwrap();
            let address = packet.as_slice().as_ptr();
            packet.prepend(&[0x45; 20]).unwrap();
            packet.prepend(&[0xee; 14]).unwrap();
            packet.pad_to(60).unwrap();
            assert_eq!(packet.as_slice()[34..].as_ptr(), address);
            let frame = packet.into_vec();
            assert_eq!(frame[34..].as_ptr(), address);
            frame
        });
        assert_eq!((allocs, reallocs), (1, 0));
        assert_eq!(&frame[34..34 + size], payload.as_slice());
        assert!(frame[34 + size..].iter().all(|byte| *byte == 0));
    }
    println!("TCP/IP/Ethernet buffer: one allocation, zero reallocations, stable payload address");
}

#[test]
fn owned_packet_remains_valid_after_producer_and_input_are_dropped() {
    let input = vec![0x77; 1460];
    let packet = owned_packet::PacketBuffer::from_slice(&input, 34).unwrap();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(packet);
    drop(input);
    let mut packet = queue.pop_front().unwrap();
    packet.prepend(&[1; 20]).unwrap();
    packet.prepend(&[2; 14]).unwrap();
    let frame = packet.into_vec();
    assert_eq!(&frame[34..], &[0x77; 1460]);
}

#[test]
fn tcp_options_and_layer_headers_share_the_payload_allocation() {
    for option_len in [0, 4, 12, 40] {
        for payload_len in [1, 1460] {
            let payload: Vec<u8> = (0..payload_len).map(|n| n as u8).collect();
            let options = vec![0x99; option_len];
            let (frame, allocs, reallocs) = counted(|| {
                let mut packet = owned_packet::PacketBuffer::from_slice(
                    &payload, 34 + 20 + option_len,
                ).unwrap();
                let address = packet.as_slice().as_ptr();
                packet.prepend(&options).unwrap();
                packet.prepend(&[0x10; 20]).unwrap();
                packet.prepend(&[0x45; 20]).unwrap();
                packet.prepend(&[0xee; 14]).unwrap();
                packet.pad_to(60).unwrap();
                let frame = packet.into_vec();
                assert_eq!(frame[54 + option_len..].as_ptr(), address);
                frame
            });
            assert_eq!((allocs, reallocs), (1, 0));
            assert_eq!(&frame[..14], &[0xee; 14]);
            assert_eq!(&frame[14..34], &[0x45; 20]);
            assert_eq!(&frame[34..54], &[0x10; 20]);
            assert_eq!(&frame[54..54 + option_len], options.as_slice());
            assert_eq!(&frame[54 + option_len..54 + option_len + payload_len], payload.as_slice());
            assert!(frame[54 + option_len + payload_len..].iter().all(|b| *b == 0));
        }
    }
}
