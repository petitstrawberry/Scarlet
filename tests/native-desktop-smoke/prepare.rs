//! Prepare disposable tmpfs documents, then run the real desktop service manager.
//! The bootstrap init has already mounted /tmp before executing this program.
use std::{fs, io::Write};

fn main() {
    let initial = fs::read("/root/fixture.txt").expect("read packaged smoke fixture");
    fs::write("/tmp/fixture.txt", &initial).expect("prepare writable tmpfs fixture");
    // Record the filesystem distinction explicitly. The small boot image uses
    // an initramfs overlay root, whose driver may not implement replacement.
    for (source, target) in [
        ("/tmp/save-probe-stage", "/tmp/fixture.txt"),
        ("/root/save-probe-stage", "/root/fixture.txt"),
    ] {
        let mut file = fs::File::create(source).expect("create disposable save probe");
        println!("SAVE_PROBE {source} write={:?}", file.write_all(&initial));
        println!("SAVE_PROBE {source} sync={:?}", file.sync_all());
        drop(file);
        println!("SAVE_PROBE {source} rename={:?}", fs::rename(source, target));
    }
    let status = std::process::Command::new("/bin/stemd")
        .status()
        .expect("start desktop service manager");
    println!("DESKTOP_EXIT {status:?}");
}
