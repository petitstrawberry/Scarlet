//! Package Scarlet AArch64 ELF code and its relocations for signed iOS execution.

mod linked;

use scarlet_loader_core::aot::{self, Layout, Segment};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fmt::Write,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const PAGE: usize = 16384;
const MAX_SPAN: usize = 64 * 1024 * 1024;
const WRAPPER: &str = r#"
.section __TEXT,__text,regular,pure_instructions
.p2align 14
.globl _scarlet_aot_rx
_scarlet_aot_rx:
.incbin "rx.bin"
.globl _scarlet_aot_rx_end
_scarlet_aot_rx_end:

.section __TEXT,__const
.globl _scarlet_aot_data
_scarlet_aot_data:
.incbin "data.bin"
.globl _scarlet_aot_data_end
_scarlet_aot_data_end:
.globl _scarlet_aot_manifest
_scarlet_aot_manifest:
.incbin "manifest"
.globl _scarlet_aot_manifest_end
_scarlet_aot_manifest_end:
"#;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let input = args.next().map(PathBuf::from);
    let output = args.next().map(PathBuf::from);
    let (Some(input), Some(output)) = (input, output) else {
        return Err(io::Error::other("usage: scarlet-ios-aot INPUT.elf OUTPUT_DIRECTORY [--lib-dir DIRECTORY]").into());
    };
    if let Some(option) = args.next() {
        if option != "--lib-dir" {
            return Err(io::Error::other("Expected --lib-dir DIRECTORY").into());
        }
        let directory = args
            .next()
            .ok_or_else(|| io::Error::other("Missing library directory"))?;
        if args.next().is_some() {
            return Err(io::Error::other("Unexpected extra arguments").into());
        }
        return linked::package(&input, &output, Path::new(&directory));
    }
    let bytes = fs::read(&input)?;
    let layout = aot::inspect_aarch64_pie(&bytes, PAGE)?;
    if layout.address_span.len() > MAX_SPAN {
        return Err(io::Error::other("PIE exceeds iOS image span limit").into());
    }
    let text = layout
        .segments
        .iter()
        .find(|s| s.permissions.execute)
        .unwrap();
    let instructions = disassemble(&input, text)?;
    if !instructions.contains(&layout.entry) {
        return Err(io::Error::other("Entry is not a disassembled AArch64 instruction").into());
    }
    package(&output, &bytes, &layout, text, &instructions)?;
    Ok(())
}

fn disassemble(path: &Path, text: &Segment) -> io::Result<BTreeSet<usize>> {
    let objdump = env::var_os("LLVM_OBJDUMP").unwrap_or_else(|| "llvm-objdump".into());
    let output = Command::new(objdump)
        .args([
            "--disassemble",
            "--no-show-raw-insn",
            "--disassembler-options=no-aliases",
        ])
        .arg(fs::canonicalize(path)?)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Disassembly failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let disassembly = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let mut instructions = BTreeSet::new();
    let bounds = text.vaddr..text.vaddr + text.file_size;
    for line in disassembly.lines() {
        let Some((address, instruction)) = line.trim().split_once(':') else {
            continue;
        };
        let Ok(address) = usize::from_str_radix(address.trim(), 16) else {
            continue;
        };
        if !bounds.contains(&address) {
            continue;
        }
        let instruction = instruction.trim();
        // LLVM renders ELF mapping-symbol data as directives. Those bytes
        // belong to the signed image but do not participate in instruction patching.
        if instruction
            .split_ascii_whitespace()
            .any(|field| matches!(field, ".word" | ".short" | ".byte" | ".quad" | ".xword"))
        {
            continue;
        }
        if instruction.is_empty()
            || instruction.contains("<unknown>")
            || !address.is_multiple_of(4)
            || address + 4 > bounds.end
            || !instructions.insert(address)
        {
            return Err(io::Error::other(format!(
                "Undecodable instruction at {address:#x}"
            )));
        }
        let operands = instruction.split(['<', ';']).next().unwrap();
        if operands
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| matches!(token, "x18" | "w18"))
        {
            return Err(io::Error::other(format!(
                "Reserved x18 is used at {address:#x}"
            )));
        }
    }
    Ok(instructions)
}

fn patch(word: u32) -> Option<u32> {
    if word & 0xffe0_001f == 0xd400_0001 {
        return Some(0xd420_0000 | (0x5300 << 5));
    }
    let breakpoint = match word & !31 {
        0xd53b_d040 => 0x5400, // MRS Xt, TPIDR_EL0
        0xd51b_d040 => 0x5500, // MSR TPIDR_EL0, Xt
        _ => return None,
    };
    Some(0xd420_0000 | (breakpoint << 5))
}

fn package(
    output: &Path,
    bytes: &[u8],
    layout: &Layout,
    text: &Segment,
    instructions: &BTreeSet<usize>,
) -> Result<(), Box<dyn Error>> {
    let origin = layout.address_span.start;
    let mut manifest = String::from("scarlet-ios-image 1\nabi scarlet\n");
    writeln!(manifest, "sha256 {:x}", Sha256::digest(bytes))?;
    writeln!(manifest, "span {}", layout.address_span.len())?;
    writeln!(manifest, "entry {}", layout.entry - origin)?;
    if let Some((address, count)) = layout.program_headers {
        writeln!(manifest, "phdr {} {count}", address - origin)?;
    }

    let mut payload = Vec::new();
    for segment in &layout.segments {
        let permission = if segment.permissions.execute {
            "rx"
        } else if segment.permissions.write {
            "rw"
        } else {
            "ro"
        };
        writeln!(
            manifest,
            "segment {permission} {} {} {} {}",
            segment.vaddr - origin,
            segment.file_size,
            segment.memory_size,
            payload.len()
        )?;
        if !segment.permissions.execute {
            payload.extend_from_slice(&bytes[segment.offset..segment.offset + segment.file_size]);
        }
    }
    if let Some(relro) = &layout.relro {
        writeln!(
            manifest,
            "relro {} {}",
            relro.start - origin,
            relro.end - origin
        )?;
    }
    if let Some((tls, align)) = &layout.tls {
        writeln!(
            manifest,
            "tls {} {} {} {align}",
            tls.vaddr - origin,
            tls.file_size,
            tls.memory_size
        )?;
    }
    for &(target, value) in &layout.relative_pointers {
        writeln!(manifest, "reloc {} {}", target - origin, value - origin)?;
    }

    let mut rx = Vec::new();
    append_text(
        &mut rx,
        &mut manifest,
        text.vaddr - origin,
        text,
        bytes,
        instructions,
    )?;
    write_payload(output, &rx, &payload, &manifest)
}

fn append_text(
    rx: &mut Vec<u8>,
    manifest: &mut String,
    address: usize,
    text: &Segment,
    bytes: &[u8],
    instructions: &BTreeSet<usize>,
) -> Result<(), Box<dyn Error>> {
    let padding = address % PAGE;
    let start = rx.len() + padding;
    rx.resize(
        rx.len() + (padding + text.memory_size).next_multiple_of(PAGE),
        0,
    );
    rx[start..start + text.file_size]
        .copy_from_slice(&bytes[text.offset..text.offset + text.file_size]);
    for &pc in instructions {
        let offset = pc - text.vaddr;
        let instruction = &mut rx[start + offset..start + offset + 4];
        let word = u32::from_le_bytes(instruction.try_into().unwrap());
        if let Some(breakpoint) = patch(word) {
            writeln!(manifest, "patch {} {word:08x}", address + offset)?;
            instruction.copy_from_slice(&breakpoint.to_le_bytes());
        }
    }
    Ok(())
}

fn write_payload(
    output: &Path,
    rx: &[u8],
    payload: &[u8],
    manifest: &str,
) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(output)?;
    fs::write(output.join("rx.bin"), rx)?;
    fs::write(output.join("data.bin"), payload)?;
    fs::write(output.join("manifest"), manifest)?;
    fs::write(output.join("image.S"), WRAPPER)?;
    Ok(())
}
