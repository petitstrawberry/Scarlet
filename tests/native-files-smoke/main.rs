//! Run only in a disposable Scarlet environment with a writable /tmp.
#[path = "../../user/std-bin/src/filer_archive.rs"]
mod filer_archive;
#[path = "../../user/std-bin/src/filer_operations.rs"]
mod filer_operations;
#[path = "../../user/std-bin/src/filer_usage.rs"]
mod filer_usage;
#[path = "../../user/std-bin/src/filer_workers.rs"]
mod filer_workers;

use std::io::{self, Write};
use std::sync::atomic::AtomicBool;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join(format!("files-worker-smoke-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let archive = root.join("fixture.zip");
    let mut output = filer_operations::native_fs::create_new(&archive)?;
    output.write_all(include_bytes!("fixture.zip"))?;
    drop(output);
    let worker = filer_workers::spawn("files-smoke-archive", move || {
        filer_archive::inspect(&archive, &AtomicBool::new(false))
    })?;
    let index = worker
        .join()
        .map_err(|_| io::Error::other("Archive worker panicked"))?
        .map_err(io::Error::other)?;
    assert_eq!(index.entries.len(), 2);
    assert_eq!(index.total_bytes, 32);
    assert_eq!(index.children("")[0].path, "Project");
    println!("SCARLET_FILES_ARCHIVE_WORKER_OK");

    let folder = root.join("Documents");
    std::fs::create_dir(&folder)?;
    let mut note = filer_operations::native_fs::create_new(folder.join("note.txt"))?;
    note.write_all(b"fixture")?;
    drop(note);
    let worker = filer_workers::spawn("files-smoke-usage", move || {
        filer_usage::measure(&folder, &AtomicBool::new(false))
    })?;
    let usage = worker
        .join()
        .map_err(|_| io::Error::other("Usage worker panicked"))?
        .map_err(io::Error::other)?;
    assert_eq!((usage.bytes, usage.files, usage.folders), (7, 1, 1));
    assert!(usage.incomplete.is_none());
    println!("SCARLET_FILES_USAGE_WORKER_OK");

    // Remove only this fixture's known files and empty directories.
    std::fs::remove_file(root.join("fixture.zip"))?;
    std::fs::remove_file(root.join("Documents/note.txt"))?;
    std::fs::remove_dir(root.join("Documents"))?;
    std::fs::remove_dir(root)?;
    Ok(())
}

fn main() {
    std::panic::set_hook(Box::new(|information| {
        println!("SCARLET_FILES_WORKER_SMOKE_FAIL: {information}");
    }));
    match run() {
        Ok(()) => println!("SCARLET_FILES_WORKER_SMOKE_OK"),
        Err(error) => {
            println!("SCARLET_FILES_WORKER_SMOKE_FAIL: {error}");
            std::process::exit(1);
        }
    }
}
