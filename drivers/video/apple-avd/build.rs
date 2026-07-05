use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PT_LOAD: u32 = 1;
const MAX_RAW_FIRMWARE_SIZE: usize = 256 * 1024;

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
    command
        .current_dir(firmware_dir)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env("RUSTFLAGS", "-C link-arg=-Tmemory.x -C link-arg=--nmagic")
        .args([
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
    let mut min_paddr = u32::MAX;
    let mut max_paddr = 0u32;

    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(&elf, off) != PT_LOAD {
            continue;
        }
        let paddr = read_u32(&elf, off + 12);
        let filesz = read_u32(&elf, off + 16);
        if filesz == 0 {
            continue;
        }
        min_paddr = min_paddr.min(paddr);
        max_paddr = max_paddr.max(
            paddr
                .checked_add(filesz)
                .expect("firmware load segment overflow"),
        );
    }

    if min_paddr == u32::MAX || max_paddr <= min_paddr {
        panic!("apple-avd firmware ELF has no loadable segments");
    }

    let raw_len = (max_paddr - min_paddr) as usize;
    if raw_len > MAX_RAW_FIRMWARE_SIZE {
        panic!("apple-avd raw firmware image is too large: {raw_len} bytes");
    }

    let mut raw = vec![0u8; raw_len];
    for index in 0..phnum {
        let off = phoff + index * phentsize;
        if read_u32(&elf, off) != PT_LOAD {
            continue;
        }
        let file_off = read_u32(&elf, off + 4) as usize;
        let paddr = read_u32(&elf, off + 12);
        let filesz = read_u32(&elf, off + 16) as usize;
        if filesz == 0 {
            continue;
        }
        let file_end = file_off
            .checked_add(filesz)
            .expect("firmware file segment overflow");
        if file_end > elf.len() {
            panic!("apple-avd firmware segment extends past ELF file");
        }
        let dst_off = (paddr - min_paddr) as usize;
        raw[dst_off..dst_off + filesz].copy_from_slice(&elf[file_off..file_end]);
    }

    validate_raw_firmware(&raw);
    fs::write(raw_path, raw).expect("failed to write apple-avd raw firmware image");
}

fn validate_raw_firmware(raw: &[u8]) {
    if raw.len() < 8 {
        panic!(
            "apple-avd raw firmware image is too small: {} bytes",
            raw.len()
        );
    }
    if raw.get(0..4) == Some(b"\x7fELF") {
        panic!("apple-avd firmware conversion produced an ELF header");
    }

    let stack_pointer = read_u32(raw, 0);
    if (stack_pointer & 0xff00_0000) != 0x2000_0000 {
        panic!("apple-avd firmware has invalid initial stack pointer: 0x{stack_pointer:08x}");
    }

    let reset_vector = read_u32(raw, 4);
    let reset_addr = reset_vector & !1;
    if (reset_vector & 1) == 0 || reset_addr as usize >= raw.len() {
        panic!("apple-avd firmware has invalid reset vector: 0x{reset_vector:08x}");
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("u16 range"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32 range"))
}
