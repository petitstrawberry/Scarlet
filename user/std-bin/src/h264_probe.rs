use rust_h264::decoder::Decoder;
use rust_h264::nal::parse_annex_b;
use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    println!("h264-probe: Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        println!("usage: h264-probe FILE.h264");
        return ExitCode::from(1);
    }

    let data = match fs::read(&args[1]) {
        Ok(data) => data,
        Err(err) => {
            println!("h264-probe: {}: {err}", args[1]);
            return ExitCode::from(1);
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
                println!("decode error at NAL {index}: {err}");
                return ExitCode::from(1);
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
        println!("decoded {frames} frames, first size {width}x{height}");
    } else {
        println!("decoded 0 frames");
    }

    ExitCode::SUCCESS
}
