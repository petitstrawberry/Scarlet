use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

pub const ELFMAG: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ELFDATA2MSB: u8 = 2;

pub const ET_REL: u16 = 1;

pub const EM_AARCH64: u16 = 183;
pub const EM_RISCV: u16 = 243;

pub const SHT_NULL: u32 = 0;
pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_STRTAB: u32 = 3;
pub const SHT_RELA: u32 = 4;
pub const SHT_NOBITS: u32 = 8;
pub const SHT_REL: u32 = 9;

pub const SHF_WRITE: u64 = 0x1;
pub const SHF_ALLOC: u64 = 0x2;
pub const SHF_EXECINSTR: u64 = 0x4;

pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;

pub const STT_NOTYPE: u8 = 0;
pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const STT_SECTION: u8 = 3;
pub const STT_FILE: u8 = 4;

pub const SHN_UNDEF: u16 = 0;

macro_rules! ELF64_R_SYM {
    ($r_info:expr) => {
        (($r_info >> 32) & 0xFFFF_FFFF) as u32
    };
}

macro_rules! ELF64_R_TYPE {
    ($r_info:expr) => {
        ($r_info & 0xFFFF_FFFF) as u32
    };
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct Elf64_Shdr {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_info: u32,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct Elf64_Sym {
    pub st_name: u32,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: u16,
    pub st_value: u64,
    pub st_size: u64,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct Elf64_Rela {
    pub r_offset: u64,
    pub r_info: u64,
    pub r_addend: i64,
}

#[derive(Debug, Clone)]
pub struct ParsedSymbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    pub bind: u8,
    pub typ: u8,
    pub shndx: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct RelocationEntry {
    pub r_offset: u64,
    pub r_sym: u32,
    pub r_type: u32,
    pub r_addend: i64,
}

#[derive(Debug, Clone)]
pub struct RelocationSection {
    pub name: String,
    pub section_index: u16,
    pub target_section_index: u16,
    pub symtab_section_index: u16,
    pub entries: Vec<RelocationEntry>,
}

#[derive(Debug, Clone)]
pub struct SectionInfo {
    pub name: String,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_addralign: u64,
    pub sh_entsize: u64,
    pub data_index: usize,
}

#[derive(Debug, Clone)]
pub struct RelocObject {
    pub is_little_endian: bool,
    pub e_machine: u16,
    pub sections: Vec<SectionInfo>,
    pub section_data: Vec<Vec<u8>>,
    pub symbols: Vec<ParsedSymbol>,
    pub relocation_sections: Vec<RelocationSection>,
    pub symtab_section_index: u16,
    pub strtab_section_index: u16,
}

const EI_MAG0: usize = 0;
const EI_MAG1: usize = 1;
const EI_MAG2: usize = 2;
const EI_MAG3: usize = 3;
const EI_CLASS: usize = 4;
const EI_DATA: usize = 5;

const ELF64_EHDR_SIZE: usize = 64;
const ELF64_SHDR_SIZE: usize = 64;
pub const ELF64_SYM_SIZE: usize = 24;
const ELF64_RELA_SIZE: usize = 24;

#[derive(Debug, Clone, Copy)]
#[allow(non_camel_case_types)]
struct Elf64_Ehdr {
    e_type: u16,
    e_machine: u16,
    e_shoff: u64,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

fn checked_range(
    data_len: usize,
    offset: usize,
    size: usize,
) -> Result<(usize, usize), &'static str> {
    let end = offset
        .checked_add(size)
        .ok_or("Offset overflow while parsing ELF")?;
    if end > data_len {
        return Err("ELF data out of bounds");
    }
    Ok((offset, end))
}

fn read_u16(buffer: &[u8], offset: usize, is_little_endian: bool) -> Result<u16, &'static str> {
    let (_, end) = checked_range(buffer.len(), offset, 2)?;
    let bytes: [u8; 2] = buffer[offset..end]
        .try_into()
        .map_err(|_| "Failed to parse u16")?;
    Ok(if is_little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(buffer: &[u8], offset: usize, is_little_endian: bool) -> Result<u32, &'static str> {
    let (_, end) = checked_range(buffer.len(), offset, 4)?;
    let bytes: [u8; 4] = buffer[offset..end]
        .try_into()
        .map_err(|_| "Failed to parse u32")?;
    Ok(if is_little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn read_u64(buffer: &[u8], offset: usize, is_little_endian: bool) -> Result<u64, &'static str> {
    let (_, end) = checked_range(buffer.len(), offset, 8)?;
    let bytes: [u8; 8] = buffer[offset..end]
        .try_into()
        .map_err(|_| "Failed to parse u64")?;
    Ok(if is_little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

fn read_i64(buffer: &[u8], offset: usize, is_little_endian: bool) -> Result<i64, &'static str> {
    let (_, end) = checked_range(buffer.len(), offset, 8)?;
    let bytes: [u8; 8] = buffer[offset..end]
        .try_into()
        .map_err(|_| "Failed to parse i64")?;
    Ok(if is_little_endian {
        i64::from_le_bytes(bytes)
    } else {
        i64::from_be_bytes(bytes)
    })
}

fn parse_elf_header(data: &[u8]) -> Result<(Elf64_Ehdr, bool), &'static str> {
    if data.len() < ELF64_EHDR_SIZE {
        return Err("ELF header too small");
    }

    if data[EI_MAG0] != ELFMAG[0]
        || data[EI_MAG1] != ELFMAG[1]
        || data[EI_MAG2] != ELFMAG[2]
        || data[EI_MAG3] != ELFMAG[3]
    {
        return Err("Invalid ELF magic");
    }

    if data[EI_CLASS] != ELFCLASS64 {
        return Err("Only ELF64 is supported");
    }

    let is_little_endian = match data[EI_DATA] {
        ELFDATA2LSB => true,
        ELFDATA2MSB => false,
        _ => return Err("Unsupported ELF endianness"),
    };

    let e_type = read_u16(data, 16, is_little_endian)?;
    if e_type != ET_REL {
        return Err("ELF is not relocatable (ET_REL expected)");
    }

    let e_machine = read_u16(data, 18, is_little_endian)?;

    let e_shoff = read_u64(data, 40, is_little_endian)?;
    let e_shentsize = read_u16(data, 58, is_little_endian)?;
    let e_shnum = read_u16(data, 60, is_little_endian)?;
    let e_shstrndx = read_u16(data, 62, is_little_endian)?;

    if e_shoff == 0 {
        return Err("Missing section header table");
    }
    if e_shentsize as usize != ELF64_SHDR_SIZE {
        return Err("Unexpected section header entry size");
    }
    if e_shnum == 0 {
        return Err("No section headers in ELF object");
    }

    Ok((
        Elf64_Ehdr {
            e_type,
            e_machine,
            e_shoff,
            e_shentsize,
            e_shnum,
            e_shstrndx,
        },
        is_little_endian,
    ))
}

fn parse_shdr(
    data: &[u8],
    offset: usize,
    is_little_endian: bool,
) -> Result<Elf64_Shdr, &'static str> {
    Ok(Elf64_Shdr {
        sh_name: read_u32(data, offset, is_little_endian)?,
        sh_type: read_u32(data, offset + 4, is_little_endian)?,
        sh_flags: read_u64(data, offset + 8, is_little_endian)?,
        sh_addr: read_u64(data, offset + 16, is_little_endian)?,
        sh_offset: read_u64(data, offset + 24, is_little_endian)?,
        sh_size: read_u64(data, offset + 32, is_little_endian)?,
        sh_link: read_u32(data, offset + 40, is_little_endian)?,
        sh_info: read_u32(data, offset + 44, is_little_endian)?,
        sh_addralign: read_u64(data, offset + 48, is_little_endian)?,
        sh_entsize: read_u64(data, offset + 56, is_little_endian)?,
    })
}

pub fn parse_symtab_entry(
    data: &[u8],
    offset: usize,
    is_little_endian: bool,
) -> Result<Elf64_Sym, &'static str> {
    Ok(Elf64_Sym {
        st_name: read_u32(data, offset, is_little_endian)?,
        st_info: data
            .get(offset + 4)
            .copied()
            .ok_or("ELF symbol entry out of bounds")?,
        st_other: data
            .get(offset + 5)
            .copied()
            .ok_or("ELF symbol entry out of bounds")?,
        st_shndx: read_u16(data, offset + 6, is_little_endian)?,
        st_value: read_u64(data, offset + 8, is_little_endian)?,
        st_size: read_u64(data, offset + 16, is_little_endian)?,
    })
}

fn parse_rela(
    data: &[u8],
    offset: usize,
    is_little_endian: bool,
) -> Result<Elf64_Rela, &'static str> {
    Ok(Elf64_Rela {
        r_offset: read_u64(data, offset, is_little_endian)?,
        r_info: read_u64(data, offset + 8, is_little_endian)?,
        r_addend: read_i64(data, offset + 16, is_little_endian)?,
    })
}

fn get_string_at(table: &[u8], offset: u32) -> Result<String, &'static str> {
    let mut idx = offset as usize;
    if idx >= table.len() {
        return Err("String table offset out of bounds");
    }

    let start = idx;
    while idx < table.len() && table[idx] != 0 {
        idx += 1;
    }

    let bytes = &table[start..idx];
    let s = core::str::from_utf8(bytes).map_err(|_| "Invalid UTF-8 in string table")?;
    Ok(String::from(s))
}

fn get_section_data(data: &[u8], shdr: &Elf64_Shdr) -> Result<Vec<u8>, &'static str> {
    if shdr.sh_type == SHT_NOBITS || shdr.sh_size == 0 {
        return Ok(Vec::new());
    }

    let offset =
        usize::try_from(shdr.sh_offset).map_err(|_| "Section offset does not fit usize")?;
    let size = usize::try_from(shdr.sh_size).map_err(|_| "Section size does not fit usize")?;
    let (start, end) = checked_range(data.len(), offset, size)?;
    Ok(data[start..end].to_vec())
}

pub fn parse_reloc_object(data: &[u8]) -> Result<RelocObject, &'static str> {
    let (ehdr, is_little_endian) = parse_elf_header(data)?;

    let shoff = usize::try_from(ehdr.e_shoff).map_err(|_| "Section header offset too large")?;
    let shentsize = ehdr.e_shentsize as usize;
    let shnum = ehdr.e_shnum as usize;

    let sh_table_size = shentsize
        .checked_mul(shnum)
        .ok_or("Section header table size overflow")?;
    checked_range(data.len(), shoff, sh_table_size)?;

    let mut section_headers = Vec::with_capacity(shnum);
    for idx in 0..shnum {
        let off = shoff + idx * shentsize;
        section_headers.push(parse_shdr(data, off, is_little_endian)?);
    }

    if ehdr.e_shstrndx as usize >= section_headers.len() {
        return Err("e_shstrndx out of bounds");
    }

    let shstr_hdr = &section_headers[ehdr.e_shstrndx as usize];
    if shstr_hdr.sh_type != SHT_STRTAB {
        return Err("Section header string table has invalid type");
    }
    let shstr_data = get_section_data(data, shstr_hdr)?;

    let mut section_data = Vec::with_capacity(section_headers.len());
    let mut sections = Vec::with_capacity(section_headers.len());

    for shdr in &section_headers {
        let name = get_string_at(&shstr_data, shdr.sh_name)?;
        let content = get_section_data(data, shdr)?;
        let data_index = section_data.len();
        section_data.push(content);
        sections.push(SectionInfo {
            name,
            sh_type: shdr.sh_type,
            sh_flags: shdr.sh_flags,
            sh_addr: shdr.sh_addr,
            sh_offset: shdr.sh_offset,
            sh_size: shdr.sh_size,
            sh_addralign: shdr.sh_addralign,
            sh_entsize: shdr.sh_entsize,
            data_index,
        });
    }

    let mut symtab_section_index: Option<usize> = None;
    for (idx, section) in sections.iter().enumerate() {
        if section.sh_type == SHT_SYMTAB && section.name == ".symtab" {
            symtab_section_index = Some(idx);
            break;
        }
    }
    if symtab_section_index.is_none() {
        for (idx, section) in sections.iter().enumerate() {
            if section.sh_type == SHT_SYMTAB {
                symtab_section_index = Some(idx);
                break;
            }
        }
    }
    let symtab_idx = symtab_section_index.ok_or("Missing SHT_SYMTAB section")?;

    let symtab_shdr = &section_headers[symtab_idx];
    let strtab_idx = symtab_shdr.sh_link as usize;
    if strtab_idx >= section_headers.len() {
        return Err("Symbol table string table index out of bounds");
    }
    if section_headers[strtab_idx].sh_type != SHT_STRTAB {
        return Err("Symbol table linked section is not a string table");
    }

    let strtab_data = &section_data[sections[strtab_idx].data_index];
    let sym_data = &section_data[sections[symtab_idx].data_index];

    let sym_ent_size = if symtab_shdr.sh_entsize == 0 {
        size_of::<Elf64_Sym>()
    } else {
        usize::try_from(symtab_shdr.sh_entsize).map_err(|_| "Invalid symtab entry size")?
    };
    if sym_ent_size < ELF64_SYM_SIZE {
        return Err("Invalid symbol table entry size");
    }
    if sym_data.len() % sym_ent_size != 0 {
        return Err("Corrupted symbol table size");
    }

    let mut symbols = Vec::with_capacity(sym_data.len() / sym_ent_size);
    for idx in 0..(sym_data.len() / sym_ent_size) {
        let off = idx * sym_ent_size;
        let sym = parse_symtab_entry(sym_data, off, is_little_endian)?;
        let bind = sym.st_info >> 4;
        let typ = sym.st_info & 0x0F;
        let name = get_string_at(strtab_data, sym.st_name)?;
        symbols.push(ParsedSymbol {
            name,
            value: sym.st_value,
            size: sym.st_size,
            bind,
            typ,
            shndx: sym.st_shndx,
        });
    }

    let mut relocation_sections = Vec::new();
    for (idx, shdr) in section_headers.iter().enumerate() {
        let section_name = &sections[idx].name;

        if shdr.sh_type == SHT_REL && section_name.starts_with(".rel") {
            return Err("SHT_REL relocations are not supported; use RELA");
        }

        if shdr.sh_type != SHT_RELA || !section_name.starts_with(".rela") {
            continue;
        }

        let rela_data = &section_data[sections[idx].data_index];
        let rela_ent_size = if shdr.sh_entsize == 0 {
            size_of::<Elf64_Rela>()
        } else {
            usize::try_from(shdr.sh_entsize).map_err(|_| "Invalid RELA entry size")?
        };
        if rela_ent_size < ELF64_RELA_SIZE {
            return Err("Invalid RELA section entry size");
        }
        if rela_data.len() % rela_ent_size != 0 {
            return Err("Corrupted RELA section size");
        }

        let target_section_index =
            u16::try_from(shdr.sh_info).map_err(|_| "Relocation target section index too large")?;
        if target_section_index as usize >= sections.len() {
            return Err("Relocation target section index out of bounds");
        }

        let symtab_section_index =
            u16::try_from(shdr.sh_link).map_err(|_| "Relocation symtab index too large")?;
        if symtab_section_index as usize >= sections.len() {
            return Err("Relocation symtab section index out of bounds");
        }

        let mut entries = Vec::with_capacity(rela_data.len() / rela_ent_size);
        for rel_idx in 0..(rela_data.len() / rela_ent_size) {
            let off = rel_idx * rela_ent_size;
            let rela = parse_rela(rela_data, off, is_little_endian)?;
            entries.push(RelocationEntry {
                r_offset: rela.r_offset,
                r_sym: ELF64_R_SYM!(rela.r_info),
                r_type: ELF64_R_TYPE!(rela.r_info),
                r_addend: rela.r_addend,
            });
        }

        relocation_sections.push(RelocationSection {
            name: sections[idx].name.clone(),
            section_index: idx as u16,
            target_section_index,
            symtab_section_index,
            entries,
        });
    }

    Ok(RelocObject {
        is_little_endian,
        e_machine: ehdr.e_machine,
        sections,
        section_data,
        symbols,
        relocation_sections,
        symtab_section_index: symtab_idx as u16,
        strtab_section_index: strtab_idx as u16,
    })
}
