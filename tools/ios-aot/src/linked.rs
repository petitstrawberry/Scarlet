//! Eagerly bind a bundled Scarlet PIE and its DT_NEEDED closure before signing.
//! Use the same relocation and symbol interposition rules as scarlet-ld.
use super::{MAX_SPAN, PAGE, append_text, disassemble, write_payload};
use object::{
    LittleEndian as Le, elf,
    read::elf::{ElfFile64, FileHeader, ProgramHeader},
};
use scarlet_loader_core::{
    Error, File, LoaderContext, Machine, Permissions, Platform, aot::Segment,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

type Result<T> = std::result::Result<T, Error>;
const MAIN: &str = "<main>";
const BIAS: usize = 0x1_0000_0000;

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
                "prelink requires a Scarlet AArch64 ET_DYN object",
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

struct Memory<'a> {
    files: &'a mut BTreeMap<String, Arc<Source>>,
    directory: &'a Path,
    bias: usize,
    bytes: Vec<u8>,
    placement: BTreeMap<String, usize>,
    request: String,
    words: BTreeSet<usize>,
}

impl Memory<'_> {
    fn offset(&self, address: usize, len: usize) -> Result<std::ops::Range<usize>> {
        let start = address.checked_sub(self.bias).ok_or(Error::Overflow)?;
        let end = start
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(Error::Overflow)?;
        Ok(start..end)
    }
}

impl Platform for Memory<'_> {
    fn page_size(&self) -> usize {
        PAGE
    }

    fn read_file(&mut self, requester: Option<&str>, name: &str) -> Result<File> {
        if requester.is_some()
            && (name.is_empty()
                || matches!(name, "." | "..")
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b)))
        {
            return Err(Error::Unsupported("DT_NEEDED requires a library basename"));
        }
        if !self.files.contains_key(name) {
            let path = self
                .directory
                .join(name)
                .canonicalize()
                .map_err(|e| Error::Platform(format!("{name}: {e}")))?;
            if !path.starts_with(self.directory) {
                return Err(Error::Format("dependency is outside the library directory"));
            }
            let source = match self.files.values().find(|source| source.path == path) {
                Some(existing) => existing.clone(),
                None => Arc::new(Source::read(&path, false)?),
            };
            self.files.insert(name.into(), source);
        }
        self.request = name.into();
        Ok(File {
            identity: self.files[name]
                .path
                .to_str()
                .ok_or(Error::Format("non-UTF-8 package path"))?
                .into(),
            bytes: self.files[name].bytes.clone(),
        })
    }

    fn map(&mut self, size: usize, align: usize) -> Result<usize> {
        let start = self.bytes.len();
        let end = start
            .checked_add(size)
            .filter(|&end| end <= MAX_SPAN)
            .ok_or(Error::Overflow)?;
        if align != PAGE || size != self.files[&self.request].span || !start.is_multiple_of(PAGE) {
            return Err(Error::Unsupported("unexpected shared object placement"));
        }
        self.placement.insert(self.request.clone(), start);
        self.bytes.resize(end, 0);
        Ok(self.bias + start)
    }
    fn write(&mut self, address: usize, bytes: &[u8]) -> Result<()> {
        let range = self.offset(address, bytes.len())?;
        if bytes.len() == 8 {
            self.words.insert(range.start);
        }
        self.bytes[range].copy_from_slice(bytes);
        Ok(())
    }
    fn read(&mut self, address: usize, bytes: &mut [u8]) -> Result<()> {
        bytes.copy_from_slice(&self.bytes[self.offset(address, bytes.len())?]);
        Ok(())
    }
    fn protect(&mut self, address: usize, size: usize, _: Permissions) -> Result<()> {
        self.offset(address, size).map(|_| ())
    }
    fn unmap(&mut self, _: usize, _: usize) {}
    fn initialize_static_tls(&mut self) -> Result<()> {
        Ok(())
    }
}

struct Bound {
    bytes: Vec<u8>,
    placement: BTreeMap<String, usize>,
    words: BTreeSet<usize>,
    initializers: Vec<usize>,
}

fn bind(files: &mut BTreeMap<String, Arc<Source>>, directory: &Path, bias: usize) -> Result<Bound> {
    let main = files[MAIN].clone();
    let mut bytes = vec![0; main.span];
    for s in &main.loads {
        bytes[s.vaddr..s.vaddr + s.file_size]
            .copy_from_slice(&main.bytes[s.offset..s.offset + s.file_size]);
    }
    let memory = Memory {
        files,
        directory,
        bias,
        bytes,
        placement: BTreeMap::from([(MAIN.into(), 0)]),
        request: String::new(),
        words: BTreeSet::new(),
    };
    let mut loader = LoaderContext::new(memory, Machine::Aarch64);
    // All addresses are offsets into our checked buffer. The adopted PT_LOAD
    // contents were copied from the same immutable file given to the loader.
    let main_id = unsafe { loader.load_existing(MAIN, bias)? };
    loader.entry(main_id)?;
    let initializers = loader
        .take_pending_initializers()?
        .into_iter()
        .map(|pc| pc.checked_sub(bias).ok_or(Error::Overflow))
        .collect::<Result<_>>()?;
    let memory = loader.platform_mut();
    Ok(Bound {
        bytes: std::mem::take(&mut memory.bytes),
        placement: std::mem::take(&mut memory.placement),
        words: std::mem::take(&mut memory.words),
        initializers,
    })
}

fn emit_branch(code: &mut Vec<u32>, start: usize, target: usize, link: bool) -> Result<()> {
    let pc = start + 4 * code.len();
    let offset = target as i64 - pc as i64;
    if offset & 3 != 0 || !(-(1 << 27)..(1 << 27)).contains(&offset) {
        return Err(Error::Format("initializer branch is out of range"));
    }
    let opcode = if link { 0x9400_0000 } else { 0x1400_0000 };
    code.push(opcode | ((offset >> 2) as u32 & 0x03ff_ffff));
    Ok(())
}

pub(super) fn package(
    input: &Path,
    output: &Path,
    directory: &Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let directory = directory.canonicalize()?;
    let main = Arc::new(Source::read(&input.canonicalize()?, true)?);
    let mut files = BTreeMap::from([(MAIN.to_string(), main.clone())]);
    // An unresolved weak or absolute symbol produces a constant. Linking at
    // two biases distinguishes those values from base-dependent ELF pointers.
    let bound = bind(&mut files, &directory, BIAS)?;
    let shifted = bind(&mut files, &directory, 2 * BIAS)?;
    if bound.placement != shifted.placement || bound.initializers != shifted.initializers {
        return Err(Error::Format("inconsistent prelinked layout").into());
    }
    let startup = bound.bytes.len();
    let mut trampoline = Vec::new();
    if !bound.initializers.is_empty() {
        // The main program receives the original argc/argv, SP and link register.
        // Dependency constructors use the Scarlet C void(void) calling convention.
        trampoline.extend([0xa9bf_07e0, 0xa9bf_7bfd]); // stp x0,x1 then x29,x30, [sp,#-16]!
        for &target in &bound.initializers {
            emit_branch(&mut trampoline, startup, target, true)?;
        }
        trampoline.extend([0xa8c1_7bfd, 0xa8c1_07e0]); // matching post-indexed ldp
        emit_branch(&mut trampoline, startup, main.entry, false)?;
    }
    let code_bytes = trampoline.len() * 4;
    let span = startup
        .checked_add(code_bytes.next_multiple_of(PAGE))
        .filter(|&size| size <= MAX_SPAN)
        .ok_or(Error::Overflow)?;
    let mut manifest = format!(
        "scarlet-ios-image 2\nabi scarlet\nsha256 {:x}\nspan {}\nentry {}\n",
        Sha256::digest(&main.bytes),
        span,
        main.entry
    );
    let (phdr, count) = main
        .phdr
        .ok_or(Error::Format("main has no loadable PHDR"))?;
    writeln!(manifest, "phdr {phdr} {count}")?;
    if let Some((start, file, size, align)) = main.tls {
        writeln!(manifest, "tls {start} {file} {size} {align}")?;
    }
    for offset in &bound.words {
        let value = |image: &Bound| {
            u64::from_le_bytes(image.bytes[*offset..*offset + 8].try_into().unwrap())
        };
        let first = value(&bound);
        let second = value(&shifted);
        if second != first {
            if second.checked_sub(first) != Some(BIAS as u64) {
                return Err(Error::Format("non-relative prelinked value").into());
            }
            writeln!(
                manifest,
                "reloc {offset} {}",
                first.checked_sub(BIAS as u64).ok_or(Error::Overflow)?
            )?;
        }
    }
    for (name, file) in &files {
        if name != MAIN {
            writeln!(manifest, "needed {name} {:x}", Sha256::digest(&file.bytes))?;
        }
    }
    let mut modules: Vec<_> = bound.placement.iter().collect();
    modules.sort_by_key(|(_, offset)| **offset);
    let mut unchecked: BTreeSet<_> = bound.initializers.iter().copied().collect();
    unchecked.insert(main.entry);
    let mut rx = Vec::new();
    let mut data = Vec::new();
    for (name, &base) in modules {
        let file = &files[name];
        for s in &file.loads {
            let address = base + s.vaddr;
            if s.permissions.execute {
                let instructions = disassemble(&file.path, s)?;
                if instructions.is_empty()
                    || (name == MAIN
                        && s.contains(file.entry, 4)
                        && !instructions.contains(&file.entry))
                {
                    return Err(io::Error::other("Missing disassembled signed text").into());
                }
                unchecked.retain(|pc| {
                    !pc.checked_sub(base)
                        .is_some_and(|pc| instructions.contains(&pc))
                });
                append_text(
                    &mut rx,
                    &mut manifest,
                    address,
                    s,
                    &file.bytes,
                    &instructions,
                )?;
                writeln!(
                    manifest,
                    "segment rx {address} {} {} 0",
                    s.file_size, s.memory_size
                )?;
            } else {
                let access = if s.permissions.write { "rw" } else { "ro" };
                writeln!(
                    manifest,
                    "segment {access} {address} {} {} {}",
                    s.memory_size,
                    s.memory_size,
                    data.len()
                )?;
                data.extend_from_slice(&bound.bytes[address..address + s.memory_size]);
            }
        }
        if let Some((start, end)) = file.relro {
            writeln!(manifest, "relro {} {}", base + start, base + end)?;
        }
    }
    if !unchecked.is_empty() {
        return Err(Error::Format("entry or initializer is outside decoded instructions").into());
    }
    if !trampoline.is_empty() {
        writeln!(manifest, "startup {startup}")?;
        writeln!(manifest, "segment rx {startup} {code_bytes} {code_bytes} 0")?;
        for instruction in trampoline {
            rx.extend_from_slice(&instruction.to_le_bytes());
        }
        rx.resize(rx.len().next_multiple_of(PAGE), 0);
    }
    write_payload(output, &rx, &data, &manifest)
}
