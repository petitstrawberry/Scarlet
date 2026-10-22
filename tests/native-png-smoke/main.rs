//! Exercises production Screenshot PNG export on its actual worker budget.
#[allow(dead_code)]
#[path = "../../user/std-bin/src/screenshot/model.rs"]
mod model;
#[path = "../../user/std-bin/src/screenshot/worker.rs"]
mod worker;

use std::path::Path;

fn check_png(path: &Path, pixels: &model::Pixels) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .map_err(|e| e.to_string())?;
    let mut decoded = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("Missing PNG buffer size")?
    ];
    let info = reader.next_frame(&mut decoded).map_err(|e| e.to_string())?;
    assert_eq!((info.width, info.height), (pixels.width, pixels.height));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    for (rgba, bgra) in decoded[..info.buffer_size()]
        .chunks_exact(4)
        .zip(pixels.bgra.chunks_exact(4))
    {
        assert_eq!(rgba, &[bgra[2], bgra[1], bgra[0], bgra[3]]);
    }
    Ok(())
}

fn exercise(directory: &Path) -> Result<(), String> {
    assert_eq!(std::thread::current().name(), Some("screenshot-save"));
    let bgra = (0..128 * 96)
        .flat_map(|i| [i as u8, (i / 128) as u8, (i * 17) as u8, 255])
        .collect();
    let image = model::Pixels::new(128, 96, bgra)?;
    let mut cropped = image.crop(model::Region {
        x: 3,
        y: 5,
        width: 97,
        height: 71,
    })?;
    let path = directory.join("capture.png");
    model::save_png(&cropped, &path, false)?;
    check_png(&path, &cropped)?;
    let original = std::fs::read(&path).map_err(|e| e.to_string())?;
    assert!(model::save_png(&image, &path, false).is_err());
    assert_eq!(std::fs::read(&path).map_err(|e| e.to_string())?, original);
    cropped.arrow((1, 1), (80, 60));
    model::save_png(&cropped, &path, true)?;
    check_png(&path, &cropped)?;
    assert!(model::save_png(&cropped, &directory.join("missing/capture.png"), false).is_err());
    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("SCARLET_PNG_WORKER_SMOKE_FAIL: {info}")
    }));
    let result = (|| -> Result<(), String> {
        let directory =
            std::env::temp_dir().join(format!("screenshot-png-smoke-{}", std::process::id()));
        std::fs::create_dir(&directory).map_err(|e| e.to_string())?;
        let worker_directory = directory.clone();
        worker::spawn("screenshot-save", move || exercise(&worker_directory))
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "Worker panicked")??;
        std::fs::remove_dir(&directory).map_err(|e| e.to_string())?;
        Ok(())
    })();
    match result {
        Ok(()) => println!("SCARLET_PNG_WORKER_SMOKE_OK"),
        Err(error) => {
            eprintln!("SCARLET_PNG_WORKER_SMOKE_FAIL: {error}");
            std::process::exit(1);
        }
    }
}
