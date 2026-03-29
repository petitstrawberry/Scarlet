use std::fmt;

const IMAGE_FILE_MACHINE_ARM64: u16 = 0xAA64;
const IMAGE_NT_SIGNATURE: u32 = 0x0000_4550;
const PE32_PLUS_MAGIC: u16 = 0x20B;

#[derive(Debug)]
pub enum PeError {
    Truncated(&'static str),
    Invalid(&'static str),
    Unsupported(&'static str),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(s) => write!(f, "truncated PE data: {s}"),
            Self::Invalid(s) => write!(f, "invalid PE file: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported PE file: {s}"),
        }
    }
}

impl std::error::Error for PeError {}

#[derive(Debug, Clone)]
pub struct ExportedFunction {
    pub name: String,
    pub rva: u32,
}

#[derive(Debug, Clone)]
struct SectionHeader {
    virtual_address: u32,
    virtual_size: u32,
    raw_ptr: u32,
    raw_size: u32,
}

#[derive(Debug, Clone)]
struct ExportDirectory {
    rva: u32,
    size: u32,
    number_of_functions: u32,
    number_of_names: u32,
    address_of_functions: u32,
    address_of_names: u32,
    address_of_name_ordinals: u32,
}

pub struct PeFile {
    bytes: Vec<u8>,
    sections: Vec<SectionHeader>,
    export_dir: ExportDirectory,
    exported_functions: Vec<ExportedFunction>,
    version: Option<String>,
}

impl PeFile {
    pub fn parse(bytes: &[u8]) -> Result<Self, PeError> {
        if bytes.len() < 0x40 {
            return Err(PeError::Truncated("missing DOS header"));
        }
        let mz = read_u16(bytes, 0)?;
        if mz != 0x5A4D {
            return Err(PeError::Invalid("DOS e_magic is not MZ"));
        }
        let e_lfanew = read_u32(bytes, 0x3C)? as usize;
        if e_lfanew + 4 + 20 > bytes.len() {
            return Err(PeError::Truncated("PE header outside file"));
        }

        let pe_sig = read_u32(bytes, e_lfanew)?;
        if pe_sig != IMAGE_NT_SIGNATURE {
            return Err(PeError::Invalid("PE signature mismatch"));
        }

        let file_header = e_lfanew + 4;
        let machine = read_u16(bytes, file_header)?;
        if machine != IMAGE_FILE_MACHINE_ARM64 {
            return Err(PeError::Unsupported("Machine is not ARM64 (0xAA64)"));
        }
        let number_of_sections = read_u16(bytes, file_header + 2)? as usize;
        let size_of_optional_header = read_u16(bytes, file_header + 16)? as usize;

        let optional_header = file_header + 20;
        if optional_header + size_of_optional_header > bytes.len() {
            return Err(PeError::Truncated("optional header outside file"));
        }

        let optional_magic = read_u16(bytes, optional_header)?;
        if optional_magic != PE32_PLUS_MAGIC {
            return Err(PeError::Unsupported("optional header is not PE32+"));
        }

        if size_of_optional_header < 116 {
            return Err(PeError::Truncated(
                "optional header too small for data directories",
            ));
        }

        let number_of_rva_and_sizes = read_u32(bytes, optional_header + 108)? as usize;
        if number_of_rva_and_sizes == 0 {
            return Err(PeError::Invalid("missing export data directory"));
        }

        let data_dir_base = optional_header + 112;
        if data_dir_base + 8 > optional_header + size_of_optional_header {
            return Err(PeError::Truncated("missing export directory entry"));
        }
        let export_rva = read_u32(bytes, data_dir_base)?;
        let export_size = read_u32(bytes, data_dir_base + 4)?;
        if export_rva == 0 || export_size == 0 {
            return Err(PeError::Invalid("export directory is empty"));
        }

        let section_base = optional_header + size_of_optional_header;
        let section_table_end = section_base
            .checked_add(
                number_of_sections
                    .checked_mul(40)
                    .ok_or(PeError::Truncated("section table size overflow"))?,
            )
            .ok_or(PeError::Truncated("section table overflow"))?;
        if section_table_end > bytes.len() {
            return Err(PeError::Truncated("section headers outside file"));
        }

        let mut sections = Vec::with_capacity(number_of_sections);
        for i in 0..number_of_sections {
            let off = section_base + i * 40;
            sections.push(SectionHeader {
                virtual_size: read_u32(bytes, off + 8)?,
                virtual_address: read_u32(bytes, off + 12)?,
                raw_size: read_u32(bytes, off + 16)?,
                raw_ptr: read_u32(bytes, off + 20)?,
            });
        }

        let export_off = rva_to_offset_impl(&sections, export_rva)
            .ok_or(PeError::Invalid("cannot map export directory RVA"))?;
        if export_off + 40 > bytes.len() {
            return Err(PeError::Truncated("export directory truncated"));
        }

        let export_dir = ExportDirectory {
            rva: export_rva,
            size: export_size,
            number_of_functions: read_u32(bytes, export_off + 20)?,
            number_of_names: read_u32(bytes, export_off + 24)?,
            address_of_functions: read_u32(bytes, export_off + 28)?,
            address_of_names: read_u32(bytes, export_off + 32)?,
            address_of_name_ordinals: read_u32(bytes, export_off + 36)?,
        };

        let mut pe = Self {
            bytes: bytes.to_vec(),
            sections,
            export_dir,
            exported_functions: Vec::new(),
            version: None,
        };

        pe.exported_functions = pe.parse_exports()?;
        pe.version = extract_version_string(&pe.bytes);
        Ok(pe)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        rva_to_offset_impl(&self.sections, rva)
    }

    pub fn exported_functions(&self) -> &[ExportedFunction] {
        &self.exported_functions
    }

    pub fn version_string(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn parse_exports(&self) -> Result<Vec<ExportedFunction>, PeError> {
        let names_off = self
            .rva_to_offset(self.export_dir.address_of_names)
            .ok_or(PeError::Invalid("cannot map AddressOfNames"))?;
        let ord_off = self
            .rva_to_offset(self.export_dir.address_of_name_ordinals)
            .ok_or(PeError::Invalid("cannot map AddressOfNameOrdinals"))?;
        let funcs_off = self
            .rva_to_offset(self.export_dir.address_of_functions)
            .ok_or(PeError::Invalid("cannot map AddressOfFunctions"))?;

        let names_count = self.export_dir.number_of_names as usize;
        let funcs_count = self.export_dir.number_of_functions as usize;

        let names_end = names_off
            .checked_add(
                names_count
                    .checked_mul(4)
                    .ok_or(PeError::Truncated("names array size overflow"))?,
            )
            .ok_or(PeError::Truncated("names array overflow"))?;
        let ord_end = ord_off
            .checked_add(
                names_count
                    .checked_mul(2)
                    .ok_or(PeError::Truncated("ordinals array size overflow"))?,
            )
            .ok_or(PeError::Truncated("ordinals array overflow"))?;
        let funcs_end = funcs_off
            .checked_add(
                funcs_count
                    .checked_mul(4)
                    .ok_or(PeError::Truncated("functions array size overflow"))?,
            )
            .ok_or(PeError::Truncated("functions array overflow"))?;

        if names_end > self.bytes.len()
            || ord_end > self.bytes.len()
            || funcs_end > self.bytes.len()
        {
            return Err(PeError::Truncated("export arrays outside file"));
        }

        let export_start = self.export_dir.rva;
        let export_end = self.export_dir.rva.saturating_add(self.export_dir.size);

        let mut out = Vec::new();
        for i in 0..names_count {
            let name_rva = read_u32(&self.bytes, names_off + i * 4)?;
            let name_off = match self.rva_to_offset(name_rva) {
                Some(v) => v,
                None => continue,
            };

            let name = match read_c_string(&self.bytes, name_off) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if !(name.starts_with("Nt") || name.starts_with("Zw")) {
                continue;
            }

            let ordinal_index = read_u16(&self.bytes, ord_off + i * 2)? as usize;
            if ordinal_index >= funcs_count {
                continue;
            }

            let func_rva = read_u32(&self.bytes, funcs_off + ordinal_index * 4)?;
            if func_rva == 0 {
                continue;
            }

            if func_rva >= export_start && func_rva < export_end {
                continue;
            }

            out.push(ExportedFunction {
                name,
                rva: func_rva,
            });
        }

        out.sort_by(|a, b| a.rva.cmp(&b.rva).then_with(|| a.name.cmp(&b.name)));
        out.dedup_by(|a, b| a.name == b.name && a.rva == b.rva);
        Ok(out)
    }
}

fn rva_to_offset_impl(sections: &[SectionHeader], rva: u32) -> Option<usize> {
    for section in sections {
        let va = section.virtual_address;
        let span = section.virtual_size.max(section.raw_size);
        if span == 0 {
            continue;
        }
        let end = va.checked_add(span)?;
        if rva < va || rva >= end {
            continue;
        }
        let delta = rva - va;
        if delta >= section.raw_size {
            return None;
        }
        let file_off = section.raw_ptr.checked_add(delta)?;
        return usize::try_from(file_off).ok();
    }
    usize::try_from(rva).ok()
}

fn extract_version_string(bytes: &[u8]) -> Option<String> {
    let key_utf16: [u16; 12] = [
        0x0046, 0x0069, 0x006C, 0x0065, 0x0056, 0x0065, 0x0072, 0x0073, 0x0069, 0x006F, 0x006E,
        0x0000,
    ];
    let mut key = Vec::with_capacity(key_utf16.len() * 2);
    for ch in key_utf16 {
        key.extend_from_slice(&ch.to_le_bytes());
    }

    let pos = find_subsequence(bytes, &key)?;
    let mut utf16 = Vec::new();
    let mut i = pos + key.len();

    while i + 1 < bytes.len() {
        let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        if u == 0 {
            if !utf16.is_empty() {
                break;
            }
            i += 2;
            continue;
        }
        let ch = char::from_u32(u as u32)?;
        if ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == ' ' {
            utf16.push(u);
            i += 2;
            continue;
        }
        if !utf16.is_empty() {
            break;
        }
        i += 2;
    }

    if utf16.is_empty() {
        return None;
    }
    let s = String::from_utf16(&utf16).ok()?;
    let compact = s.trim().replace(',', ".").replace(' ', "");
    if compact.chars().any(|c| c.is_ascii_digit()) {
        Some(compact)
    } else {
        None
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let mut i = 0usize;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn read_c_string(bytes: &[u8], off: usize) -> Result<String, PeError> {
    if off >= bytes.len() {
        return Err(PeError::Truncated("string offset outside file"));
    }
    let mut end = off;
    while end < bytes.len() && bytes[end] != 0 {
        end += 1;
    }
    if end == bytes.len() {
        return Err(PeError::Truncated("unterminated export string"));
    }
    let s = std::str::from_utf8(&bytes[off..end])
        .map_err(|_| PeError::Invalid("non-utf8 export name"))?;
    Ok(s.to_string())
}

fn read_u16(bytes: &[u8], off: usize) -> Result<u16, PeError> {
    let end = off
        .checked_add(2)
        .ok_or(PeError::Truncated("u16 offset overflow"))?;
    let src = bytes
        .get(off..end)
        .ok_or(PeError::Truncated("u16 out of bounds"))?;
    Ok(u16::from_le_bytes([src[0], src[1]]))
}

fn read_u32(bytes: &[u8], off: usize) -> Result<u32, PeError> {
    let end = off
        .checked_add(4)
        .ok_or(PeError::Truncated("u32 offset overflow"))?;
    let src = bytes
        .get(off..end)
        .ok_or(PeError::Truncated("u32 out of bounds"))?;
    Ok(u32::from_le_bytes([src[0], src[1], src[2], src[3]]))
}
