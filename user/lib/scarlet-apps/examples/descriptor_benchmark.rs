//! Host benchmark for bounded shared package descriptor validation.
use scarlet_apps as desktop;

fn main() {
    let root = std::env::temp_dir().join(format!("astra-app-descriptor-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let mut elf = [0u8; 20];
    elf[..8].copy_from_slice(b"\x7fELF\x02\x01\x01\x53");
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    elf[18..20].copy_from_slice(&183u16.to_le_bytes());
    std::fs::write(root.join("executable"), elf).unwrap();
    for count in [1_000, 10_000, 50_000] {
        let mut text = String::from(
            "[Desktop Entry]\nName=Benchmark\nType=Application\nExec=executable\nX-Scarlet-AppFormat=1\nX-Scarlet-Target=aarch64-unknown-scarlet\n",
        );
        for index in 0..count {
            text.push_str(&format!("X-{index:06}=x\n"));
        }
        std::fs::write(root.join("org.benchmark.desktop"), &text).unwrap();
        let start = std::time::Instant::now();
        let result = desktop::validate_app_directory(root.to_str().unwrap());
        println!(
            "keys={count} bytes={} elapsed={:?} valid={}",
            text.len(),
            start.elapsed(),
            result.is_ok()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
