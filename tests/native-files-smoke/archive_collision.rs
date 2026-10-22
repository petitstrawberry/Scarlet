//! Boot only with the documented disposable, packaged lower-layer fixture.
#[path = "../../user/std-bin/src/filer/archive.rs"]
mod archive;
#[path = "../../user/std-bin/src/filer/operations.rs"]
mod operations;
#[path = "../../user/std-bin/src/filer/workers.rs"]
mod workers;

use std::{fs, io, path::Path, sync::atomic::AtomicBool};

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = Path::new("/root/archive-collision-smoke");
    assert_eq!(
        fs::read(root.join("fixture-marker"))?,
        b"isolated-archive-collision-v1\n"
    );
    assert_eq!(
        fs::read(root.join("lower-directory/keep.txt"))?,
        b"keep lower directory\n"
    );
    assert_eq!(fs::read(root.join("lower-file"))?, b"keep lower file\n");
    assert!(
        fs::symlink_metadata(root.join("lower-link"))?
            .file_type()
            .is_symlink()
    );
    let archive = root.join("fixture.zip");
    let cancelled = AtomicBool::new(false);
    let index = archive::inspect(&archive, &cancelled).map_err(io::Error::other)?;
    // Check lower entries before any successful creation copies the parent up.
    for name in ["lower-directory", "lower-file", "lower-link"] {
        let error = archive::extract(&archive, root, name, &index, &cancelled)
            .expect_err("extraction must reject an existing lower destination");
        assert!(
            error.contains("Destination already exists"),
            "{name}: {error}"
        );
        println!("SCARLET_ARCHIVE_COLLISION_REJECTED {name}");
    }
    assert_eq!(
        fs::read(root.join("lower-directory/keep.txt"))?,
        b"keep lower directory\n"
    );
    assert_eq!(fs::read_dir(root.join("lower-directory"))?.count(), 1);
    assert_eq!(fs::read(root.join("lower-file"))?, b"keep lower file\n");
    assert_eq!(
        fs::read_link(root.join("lower-link"))?,
        Path::new("missing-target")
    );
    let destination = archive::extract(&archive, root, "new-directory", &index, &cancelled)
        .map_err(io::Error::other)?;
    for entry in &index.entries {
        if !entry.is_directory {
            assert_eq!(
                fs::metadata(destination.join(&entry.path))?.len(),
                entry.size
            );
        }
    }
    let error = archive::extract(&archive, root, "new-directory", &index, &cancelled)
        .expect_err("second extraction must reject an existing upper destination");
    assert!(error.contains("Destination already exists"), "{error}");
    println!("SCARLET_ARCHIVE_COLLISION_UPPER_REJECTED");
    Ok(())
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        println!("SCARLET_ARCHIVE_COLLISION_SMOKE_FAIL: {info}")
    }));
    // Exercise the same bounded worker stack as the real Files application.
    let result = workers::spawn("archive-collision", run)
        .map_err(|e| e.to_string())
        .and_then(|worker| worker.join().map_err(|_| "worker panicked".to_string()))
        .and_then(|result| result.map_err(|e| e.to_string()));
    match result {
        Ok(()) => println!("SCARLET_ARCHIVE_COLLISION_SMOKE_OK"),
        Err(error) => {
            println!("SCARLET_ARCHIVE_COLLISION_SMOKE_FAIL: {error}");
            std::process::exit(1);
        }
    }
}
