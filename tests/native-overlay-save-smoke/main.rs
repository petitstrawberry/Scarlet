//! Run only in an isolated guest containing the matching packaged fixture marker.
#![allow(dead_code)]
#[path = "../../user/std-bin/src/notepad/document.rs"]
mod document;
use scarlet_fs as native_fs;

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

fn bytes(file: &mut File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut result = Vec::new();
    file.read_to_end(&mut result)?;
    Ok(result)
}

#[cfg(target_os = "scarlet")]
fn handle_id(file: &File) -> u64 {
    use scarlet_sys::{RawFileMetadata, Syscall, syscall2};
    use std::os::fd::AsRawFd;
    let mut metadata = std::mem::MaybeUninit::<RawFileMetadata>::uninit();
    // SAFETY: the writable output has the exact ABI size and remains live for
    // this synchronous call. It is read only after a successful full write.
    assert_eq!(
        unsafe {
            syscall2(
                Syscall::FileMetadata,
                file.as_raw_fd() as usize,
                metadata.as_mut_ptr() as usize,
            )
        },
        0
    );
    unsafe { metadata.assume_init() }.file_id
}

#[cfg(target_os = "scarlet")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new("/root/native-overlay-save-smoke");
    assert_eq!(
        fs::read(root.join("fixture-marker"))?,
        b"isolated-native-overlay-save-v1\n"
    );
    let target = root.join("lower-target.txt");
    let lower_source = root.join("lower-source.txt");
    assert_eq!(fs::read(&target)?, b"packaged lower target\n");
    let mut old_destination = File::open(&target)?;
    let old_destination_id = native_fs::metadata(&target)?.file_id();

    let source = root.join("upper-stage");
    let mut writer = native_fs::create_new(&source)?;
    writer.write_all(b"first save")?;
    writer.sync_all()?;
    let mut source_reader = File::open(&source)?;
    let source_id = native_fs::metadata(&source)?.file_id();
    fs::rename(&source, &target)?;
    assert!(!source.exists());
    assert_eq!(native_fs::metadata(&target)?.file_id(), source_id);
    assert_eq!(handle_id(&source_reader), source_id);
    assert_eq!(handle_id(&old_destination), old_destination_id);
    writer.write_all(b" through retained handle")?;
    writer.sync_all()?;
    assert_eq!(fs::read(&target)?, b"first save through retained handle");
    assert_eq!(
        bytes(&mut source_reader)?,
        b"first save through retained handle"
    );
    assert_eq!(bytes(&mut old_destination)?, b"packaged lower target\n");
    println!("OVERLAY_SAVE retained upper and replaced lower handles OK");

    // Use the editor's actual staged-save path twice. This catches stale node
    // locations on reopening, not merely one successful underlying rename.
    document::save_atomic(&target, "second 猫 save".as_bytes())?;
    let second_id = native_fs::metadata(&target)?.file_id();
    assert_ne!(second_id, source_id);
    assert_eq!(fs::read_to_string(&target)?, "second 猫 save");
    document::save_atomic(&target, b"third save")?;
    assert_ne!(native_fs::metadata(&target)?.file_id(), second_id);
    assert_eq!(fs::read(&target)?, b"third save");
    assert_eq!(
        bytes(&mut source_reader)?,
        b"first save through retained handle"
    );
    assert_eq!(bytes(&mut old_destination)?, b"packaged lower target\n");
    println!("OVERLAY_SAVE repeated production Notepad saves OK");

    let new_source = root.join("new-upper-source");
    let new_target = root.join("new-upper-target");
    native_fs::create_new(&new_source)?.write_all(b"upper only")?;
    let new_id = native_fs::metadata(&new_source)?.file_id();
    native_fs::rename_noreplace(&new_source, &new_target)?;
    assert_eq!(native_fs::metadata(&new_target)?.file_id(), new_id);
    assert_eq!(fs::read(&new_target)?, b"upper only");

    let collision = root.join("collision-source");
    native_fs::create_new(&collision)?.write_all(b"keep collision source")?;
    assert_eq!(
        native_fs::rename_noreplace(&collision, &new_target)
            .unwrap_err()
            .raw_os_error(),
        Some(17)
    );
    assert_eq!(fs::read(&collision)?, b"keep collision source");
    assert_eq!(fs::read(&new_target)?, b"upper only");

    assert!(fs::rename(&lower_source, root.join("moved-lower-source")).is_err());
    assert_eq!(fs::read(&lower_source)?, b"packaged lower source\n");
    assert!(!root.join("moved-lower-source").exists());
    assert!(fs::rename(&target, root.join("moved-lower-backed-target")).is_err());
    assert_eq!(fs::read(&target)?, b"third save");
    assert!(!root.join("moved-lower-backed-target").exists());

    let directory = root.join("upper-directory");
    fs::create_dir(&directory)?;
    assert!(fs::rename(&directory, root.join("moved-directory")).is_err());
    assert!(directory.is_dir());

    let hidden = root.join("whiteout-target.txt");
    fs::remove_file(&hidden)?;
    assert!(!hidden.exists());
    assert!(fs::rename(&collision, &hidden).is_err());
    assert_eq!(fs::read(&collision)?, b"keep collision source");
    assert!(!hidden.exists());
    println!("OVERLAY_SAVE no-replace and unsupported cases preserve entries OK");
    // Every path belongs to the disposable packaged fixture. The guest is
    // destroyed after this process; do not recursively remove overlay trees.
    Ok(())
}

#[cfg(target_os = "scarlet")]
fn main() {
    std::panic::set_hook(Box::new(|info| {
        println!("SCARLET_OVERLAY_SAVE_SMOKE_FAIL: {info}")
    }));
    if let Err(error) = run() {
        println!("SCARLET_OVERLAY_SAVE_SMOKE_FAIL: {error}");
        std::process::exit(1);
    }
    println!("SCARLET_OVERLAY_SAVE_SMOKE_OK");
}

#[cfg(not(target_os = "scarlet"))]
fn main() {
    panic!("This fixture must run in an isolated native Scarlet guest");
}
