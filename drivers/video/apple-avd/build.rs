use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(3)
        .expect("apple-avd driver path has repo root")
        .to_path_buf();
    let firmware_dir = repo_root.join("firmware/apple-avd-fw-rs");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let firmware_feature =
        env::var("SCARLET_APPLE_AVD_FW_FEATURE").unwrap_or_else(|_| String::from("v3-t0"));
    let raw_path = out_dir.join(format!("apple-avd-fw-{firmware_feature}.bin"));

    println!("cargo:rerun-if-env-changed=SCARLET_APPLE_AVD_FW_FEATURE");
    println!(
        "cargo:rerun-if-changed={}",
        firmware_dir.join("Cargo.toml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        firmware_dir.join("build.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        firmware_dir.join("memory.x").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        firmware_dir.join("src").display()
    );

    build_firmware(&firmware_dir, &firmware_feature);
    let elf_path = firmware_dir.join("target/thumbv7m-none-eabi/release/apple-avd-fw");
    elf_to_raw(&elf_path, &raw_path);
    println!(
        "cargo:rustc-env=SCARLET_APPLE_AVD_FW_BIN={}",
        raw_path.display()
    );
}

fn build_firmware(firmware_dir: &Path, firmware_feature: &str) {
    let mut command = Command::new("cargo");
    command.current_dir(firmware_dir).args([
        "build",
        "-Zbuild-std=core",
        "--release",
        "--no-default-features",
        "--features",
        firmware_feature,
    ]);
    let status = command
        .status()
        .expect("failed to spawn cargo for apple-avd firmware");
    if !status.success() {
        panic!("apple-avd firmware build failed with status {status}");
    }
}

fn elf_to_raw(elf_path: &Path, raw_path: &Path) {
    let elf = fs::read(elf_path).expect("failed to read apple-avd firmware ELF");
    if elf.get(0..4) != Some(b"\x7fELF") {
        panic!("apple-avd firmware is not an ELF file");
    }
    if elf.get(4).copied() != Some(1) || elf.get(5).copied() != Some(1) {
        panic!("apple-avd firmware must be ELF32 little-endian");
    }

    let phoff = read_u32(&elf, 28) as usize;
    let phentsize = read_u16(&elf, 42) as usize;
    let phnum = read_u16(&elf, 44) as usize;
    let mut min_vaddr = u32::MAX;
    let mut max_vaddr = 0u32;

    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(&elf, off) != 1 {
            continue;
        }
        let vaddr = read_u32(&elf, off + 8);
        let memsz = read_u32(&elf, off + 20);
        if memsz == 0 {
            continue;
        }
        min_vaddr = min_vaddr.min(vaddr);
        max_vaddr = max_vaddr.max(vaddr.checked_add(memsz).expect("firmware memsz overflow"));
    }

    if min_vaddr == u32::MAX || max_vaddr <= min_vaddr {
        panic!("apple-avd firmware ELF has no loadable segments");
    }

    let mut raw = vec![0u8; (max_vaddr - min_vaddr) as usize];
    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(&elf, off) != 1 {
            continue;
        }
        let file_off = read_u32(&elf, off + 4) as usize;
        let vaddr = read_u32(&elf, off + 8);
        let filesz = read_u32(&elf, off + 16) as usize;
        if filesz == 0 {
            continue;
        }
        let dst_off = (vaddr - min_vaddr) as usize;
        raw[dst_off..dst_off + filesz].copy_from_slice(&elf[file_off..file_off + filesz]);
    }

    fs::write(raw_path, raw).expect("failed to write apple-avd raw firmware image");
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("u16 range"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32 range"))
}
