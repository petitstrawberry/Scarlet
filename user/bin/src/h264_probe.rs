#![no_std]
#![no_main]

extern crate scarlet_std as std;

use rust_h264::decoder::Decoder;
use rust_h264::nal::parse_annex_b;
use std::fs::File;
use std::string::String;
use std::vec::Vec;
use std::{format, println};

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("usage: h264_probe FILE.h264");
        return 1;
    }

    let data = match read_file(&args[1]) {
        Ok(data) => data,
        Err(err) => {
            println!("h264_probe: {}: {}", args[1], err);
            return 1;
        }
    };

    let nals = parse_annex_b(&data);
    println!("input: {} bytes, {} NAL units", data.len(), nals.len());

    let mut decoder = Decoder::new();
    let mut frames = 0usize;
    let mut first_size = None;

    for (index, nal) in nals.iter().enumerate() {
        match decoder.decode_nal(nal) {
            Ok(Some(frame)) => {
                frames += 1;
                first_size.get_or_insert((frame.width, frame.height));
                println!(
                    "frame {}: {}x{}, poc={}",
                    frames, frame.width, frame.height, frame.pic_order_cnt
                );
            }
            Ok(None) => {}
            Err(err) => {
                println!("decode error at NAL {}: {}", index, err);
                return 1;
            }
        }
    }

    if let Some(frame) = decoder.flush() {
        frames += 1;
        first_size.get_or_insert((frame.width, frame.height));
        println!(
            "frame {}: {}x{}, poc={}",
            frames, frame.width, frame.height, frame.pic_order_cnt
        );
    }

    if let Some((width, height)) = first_size {
        println!("decoded {} frames, first size {}x{}", frames, width, height);
    } else {
        println!("decoded 0 frames");
    }

    0
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("open failed"))?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        let read = file.read(&mut buffer).map_err(|_| format!("read failed"))?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
    }

    Ok(data)
}
