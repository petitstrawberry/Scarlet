//! Package one dynamic ELF object. Dependencies and relocations belong to exec.
use super::{MAX_SPAN, PAGE, append_text, disassemble, write_payload};
use object::{
    LittleEndian as Le, elf,
    read::elf::{ElfFile64, FileHeader, ProgramHeader},
};
use scarlet_loader_core::{Error, Permissions, aot::Segment};
use sha2::{Digest, Sha256};
use std::{
    fmt::Write,
    fs, io,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Error>;

struct Source {
    path: PathBuf,
    bytes: Vec<u8>,
    loads: Vec<Segment>,
    span: usize,
    phdr: Option<(usize, usize)>,
    tls: Option<(usize, usize, usize, usize)>,
    relro: Option<(usize, usize)>,
    entry: usize,
}

impl Source {
    fn read(path: &Path, main: bool) -> Result<Self> {
        let bytes =
            fs::read(path).map_err(|e| Error::Platform(format!("{}: {e}", path.display())))?;
        let file = ElfFile64::<Le>::parse(&*bytes).map_err(|_| Error::Format("invalid ELF64"))?;
        let header = file.elf_header();
        if bytes[7] != b'S'
            || header.e_type(Le) != elf::ET_DYN
            || header.e_machine(Le) != elf::EM_AARCH64
        {
            return Err(Error::Unsupported(
                "packaging requires a Scarlet AArch64 ET_DYN object",
            ));
        }
        let mut loads = Vec::new();
        let mut tls = None;
        let mut relro = None;
        let mut interpreter = false;
        let mut span = 0;
        let entry = header.e_entry(Le) as usize;
        for ph in file.elf_program_headers() {
            let address = ph.p_vaddr(Le) as usize;
            let filed = ph.p_filesz(Le) as usize;
            let size = ph.p_memsz(Le) as usize;
            let end = address.checked_add(size).ok_or(Error::Overflow)?;
            match ph.p_type(Le) {
                elf::PT_LOAD if size > 0 => {
                    let offset = ph.p_offset(Le) as usize;
                    let flags = ph.p_flags(Le);
                    if end > MAX_SPAN
                        || filed > size
                        || flags & elf::PF_R == 0
                        || flags & (elf::PF_W | elf::PF_X) == (elf::PF_W | elf::PF_X)
                        || ph.p_align(Le) != PAGE as u64
                        || offset % PAGE != address % PAGE
                        || bytes
                            .get(offset..offset.checked_add(filed).ok_or(Error::Overflow)?)
                            .is_none()
                    {
                        return Err(Error::Format("invalid 16 KiB load segment"));
                    }
                    loads.push(Segment {
                        offset,
                        vaddr: address,
                        file_size: filed,
                        memory_size: size,
                        permissions: Permissions {
                            read: true,
                            write: flags & elf::PF_W != 0,
                            execute: flags & elf::PF_X != 0,
                        },
                    });
                    span = span.max(end.next_multiple_of(PAGE));
                }
                elf::PT_INTERP => {
                    if !main
                        || interpreter
                        || ph.data(Le, &*bytes).ok() != Some(b"/bin/scarlet-ld\0")
                    {
                        return Err(Error::Unsupported("unknown or duplicate ELF interpreter"));
                    }
                    interpreter = true;
                }
                elf::PT_TLS => tls = Some((address, filed, size, ph.p_align(Le).max(1) as usize)),
                elf::PT_GNU_RELRO if size > 0 => relro = Some((address, end)),
                _ => {}
            }
        }
        loads.sort_by_key(|s| s.vaddr);
        if main != interpreter
            || loads.first().is_none_or(|s| s.vaddr >= PAGE)
            || loads.windows(2).any(|pair| {
                (pair[0].vaddr + pair[0].memory_size).next_multiple_of(PAGE)
                    > pair[1].vaddr / PAGE * PAGE
            })
        {
            return Err(Error::Unsupported("incompatible PIE layout or interpreter"));
        }
        let offset = header.e_phoff(Le) as usize;
        let count = file.elf_program_headers().len();
        let phdr = loads
            .iter()
            .find(|s| {
                offset >= s.offset
                    && offset
                        .checked_add(count * 56)
                        .is_some_and(|end| end <= s.offset + s.file_size)
            })
            .map(|s| (s.vaddr + (offset - s.offset), count));
        Ok(Self {
            path: path.into(),
            bytes,
            loads,
            span,
            phdr,
            tls,
            relro,
            entry,
        })
    }
}

pub(super) fn package(
    input: &Path,
    output: &Path,
    main: bool,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let file = Source::read(input, main)?;
    let span = file.span + if main { PAGE } else { 0 };
    if file.bytes.len() > MAX_SPAN || span > MAX_SPAN || (!main && file.tls.is_some()) {
        return Err(Error::Unsupported("unsupported dynamic object span or DSO TLS").into());
    }
    let kind = if main { "pie" } else { "dso" };
    let mut manifest = format!(
        "scarlet-ios-image 3\nabi scarlet\nsha256 {:x}\nspan {span}\nentry {}\nkind {kind}\n",
        Sha256::digest(&file.bytes),
        file.entry
    );
    if let Some((phdr, count)) = file.phdr {
        writeln!(manifest, "phdr {phdr} {count}")?;
    }
    if let Some((start, filed, size, align)) = file.tls {
        writeln!(manifest, "tls {start} {filed} {size} {align}")?;
    }
    let mut rx = Vec::new();
    let mut entry_found = false;
    for s in &file.loads {
        let access = if s.permissions.execute {
            "rx"
        } else if s.permissions.write {
            "rw"
        } else {
            "ro"
        };
        writeln!(
            manifest,
            "segment {access} {} {} {} {}",
            s.vaddr, s.file_size, s.memory_size, s.offset
        )?;
        if s.permissions.execute {
            let instructions = disassemble(&file.path, s)?;
            entry_found |= instructions.contains(&file.entry);
            append_text(
                &mut rx,
                &mut manifest,
                s.vaddr,
                s,
                &file.bytes,
                &instructions,
            )?;
        }
    }
    if let Some((start, end)) = file.relro {
        writeln!(manifest, "relro {start} {end}")?;
    }
    if main {
        if !entry_found {
            return Err(io::Error::other("PIE entry is outside decoded text").into());
        }
        writeln!(manifest, "startup {}", file.span)?;
        writeln!(manifest, "segment rx {} {PAGE} {PAGE} 0", file.span)?;
    }
    write_payload(output, &rx, &file.bytes, &manifest, main)
}
