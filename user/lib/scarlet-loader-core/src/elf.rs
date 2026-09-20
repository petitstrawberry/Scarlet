use crate::{Error, MAX_IMAGE_SIZE, Machine, PAGE_SIZE, Permissions};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

const MAX_ENTRIES: usize = 1024 * 1024;

pub(crate) struct Segment {
    pub offset: usize,
    pub vaddr: usize,
    pub file_size: usize,
    pub memory_size: usize,
    pub permissions: Permissions,
}
impl Segment {
    pub fn contains(&self, address: usize, size: usize) -> bool {
        address >= self.vaddr
            && address
                .checked_add(size)
                .is_some_and(|end| end <= self.vaddr + self.memory_size)
    }
}
pub(crate) struct Symbol {
    pub name: Vec<u8>,
    pub value: usize,
    pub binding: u8,
    pub visibility: u8,
    pub defined: bool,
    pub absolute: bool,
}
pub(crate) struct Relocation {
    pub offset: usize,
    pub symbol: usize,
    pub kind: u32,
    pub addend: i64,
}
pub(crate) struct Parsed {
    pub executable: bool,
    pub entry: usize,
    pub segments: Vec<Segment>,
    pub min_vaddr: usize,
    pub max_vaddr: usize,
    pub align: usize,
    pub needed: Vec<String>,
    pub symbols: Vec<Symbol>,
    pub relocations: Vec<Relocation>,
    pub init: Option<usize>,
    pub init_array: Option<(usize, usize)>,
    pub relro: Option<(usize, usize)>,
    exports: BTreeMap<Vec<u8>, usize>,
}
impl Parsed {
    pub fn export(&self, name: &[u8]) -> Option<&Symbol> {
        self.exports.get(name).map(|index| &self.symbols[*index])
    }
    pub fn parse(bytes: &[u8], machine: Machine) -> Result<Self, Error> {
        if bytes.get(..4) != Some(b"\x7fELF") {
            return Err(Error::Format("bad ELF magic"));
        }
        if bytes.get(4) != Some(&2) || bytes.get(5) != Some(&1) || bytes.get(6) != Some(&1) {
            return Err(Error::Unsupported("requires ELF64 little-endian version 1"));
        }
        if u16_at(bytes, 52)? != 64 || u32_at(bytes, 20)? != 1 {
            return Err(Error::Format("invalid ELF header"));
        }
        let executable = match u16_at(bytes, 16)? {
            2 => true,
            3 => false,
            _ => return Err(Error::Unsupported("requires ET_DYN or ET_EXEC")),
        };
        let expected = match machine {
            Machine::Aarch64 => 183,
            Machine::Riscv64 => 243,
        };
        if u16_at(bytes, 18)? != expected {
            return Err(Error::Unsupported("ELF machine does not match process"));
        }
        if u16_at(bytes, 54)? != 56 {
            return Err(Error::Format("invalid program header entry size"));
        }
        let phoff = address(bytes, 32)?;
        let phnum = u16_at(bytes, 56)? as usize;
        if phnum == 0 || phnum == 0xffff {
            return Err(Error::Unsupported("missing or extended program headers"));
        }
        range(bytes, phoff, phnum.checked_mul(56).ok_or(Error::Overflow)?)?;
        let mut parsed = Self {
            executable,
            entry: address(bytes, 24)?,
            segments: Vec::new(),
            min_vaddr: usize::MAX,
            max_vaddr: 0,
            align: PAGE_SIZE,
            needed: Vec::new(),
            symbols: Vec::new(),
            relocations: Vec::new(),
            init: None,
            init_array: None,
            relro: None,
            exports: BTreeMap::new(),
        };
        let mut dynamic = None;
        let mut relro = None;
        for i in 0..phnum {
            let ph = phoff + i * 56;
            let kind = u32_at(bytes, ph)?;
            let flags = u32_at(bytes, ph + 4)?;
            let offset = address(bytes, ph + 8)?;
            let vaddr = address(bytes, ph + 16)?;
            let file_size = address(bytes, ph + 32)?;
            let memory_size = address(bytes, ph + 40)?;
            let alignment = address(bytes, ph + 48)?;
            if kind == 7 && memory_size != 0 {
                return Err(Error::Unsupported("PT_TLS"));
            }
            if kind == 0x6474e551 && flags & 1 != 0 {
                return Err(Error::Unsupported("executable stack"));
            }
            if kind == 1 {
                if file_size > memory_size {
                    return Err(Error::Format("segment filesz exceeds memsz"));
                }
                range(bytes, offset, file_size)?;
                let end = vaddr.checked_add(memory_size).ok_or(Error::Overflow)?;
                if alignment > 1
                    && (!alignment.is_power_of_two() || vaddr % alignment != offset % alignment)
                {
                    return Err(Error::Format("invalid segment alignment"));
                }
                if flags & !7 != 0 {
                    return Err(Error::Unsupported("unknown PT_LOAD permission flags"));
                }
                let permissions = Permissions {
                    read: flags & 4 != 0,
                    write: flags & 2 != 0,
                    execute: flags & 1 != 0,
                };
                if permissions.write && permissions.execute {
                    return Err(Error::Unsupported("writable executable segment"));
                }
                if memory_size == 0 {
                    continue;
                }
                if alignment > MAX_IMAGE_SIZE {
                    return Err(Error::Unsupported("segment alignment exceeds limit"));
                }
                parsed.align = parsed.align.max(alignment);
                parsed.min_vaddr = parsed.min_vaddr.min(down(vaddr));
                parsed.max_vaddr = parsed.max_vaddr.max(up(end)?);
                parsed.segments.push(Segment {
                    offset,
                    vaddr,
                    file_size,
                    memory_size,
                    permissions,
                });
            } else if kind == 2 {
                if dynamic.is_some() {
                    return Err(Error::Format("multiple PT_DYNAMIC segments"));
                }
                range(bytes, offset, file_size)?;
                dynamic = Some((vaddr, offset, file_size));
            } else if kind == 0x6474e552 {
                if relro.is_some() {
                    return Err(Error::Format("multiple PT_GNU_RELRO segments"));
                }
                relro = Some((vaddr, memory_size));
            }
        }
        if parsed.segments.is_empty() {
            return Err(Error::Format("no loadable segments"));
        }
        if parsed.max_vaddr - parsed.min_vaddr > MAX_IMAGE_SIZE {
            return Err(Error::Unsupported("image exceeds size limit"));
        }
        parsed.segments.sort_unstable_by_key(|s| s.vaddr);
        for adjacent in parsed.segments.windows(2) {
            let a = &adjacent[0];
            let b = &adjacent[1];
            if a.vaddr + a.memory_size > b.vaddr {
                return Err(Error::Unsupported("overlapping PT_LOAD segments"));
            }
            if up(a.vaddr + a.memory_size)? > down(b.vaddr)
                && ((a.permissions.write && b.permissions.execute)
                    || (a.permissions.execute && b.permissions.write))
            {
                return Err(Error::Unsupported("writable executable shared page"));
            }
        }
        if let Some((start, size)) = relro
            && size != 0
        {
            let end = start.checked_add(size).ok_or(Error::Overflow)?;
            if !parsed.segments.iter().any(|s| s.contains(start, size)) {
                return Err(Error::Format("RELRO is outside a load segment"));
            }
            parsed.relro = Some((down(start), down(end)));
        }
        if let Some((vaddr, offset, size)) = dynamic {
            if size < 16 || size % 16 != 0 {
                return Err(Error::Format("invalid dynamic table size"));
            }
            if parsed.file_offset(vaddr, size)? != offset {
                return Err(Error::Format("dynamic table does not match load segment"));
            }
            parsed.parse_dynamic(bytes, offset, size, machine)?;
        }
        Ok(parsed)
    }
    fn file_offset(&self, vaddr: usize, size: usize) -> Result<usize, Error> {
        for segment in &self.segments {
            if vaddr >= segment.vaddr
                && vaddr
                    .checked_add(size)
                    .is_some_and(|end| end <= segment.vaddr + segment.file_size)
            {
                return segment
                    .offset
                    .checked_add(vaddr - segment.vaddr)
                    .ok_or(Error::Overflow);
            }
        }
        Err(Error::Format(
            "metadata is outside file-backed PT_LOAD bytes",
        ))
    }
    fn file_available(&self, vaddr: usize) -> Result<usize, Error> {
        self.segments
            .iter()
            .find(|s| vaddr >= s.vaddr && vaddr < s.vaddr + s.file_size)
            .map(|s| s.vaddr + s.file_size - vaddr)
            .ok_or(Error::Format(
                "metadata pointer is outside file-backed PT_LOAD bytes",
            ))
    }
    fn parse_dynamic(
        &mut self,
        bytes: &[u8],
        offset: usize,
        size: usize,
        machine: Machine,
    ) -> Result<(), Error> {
        if size / 16 > 4096 {
            return Err(Error::Unsupported("dynamic table exceeds entry limit"));
        }
        let mut tags = BTreeMap::new();
        let mut needs = Vec::new();
        let mut terminated = false;
        for i in 0..size / 16 {
            let tag = u64_at(bytes, offset + i * 16)?;
            let value = address(bytes, offset + i * 16 + 8)?;
            if tag == 0 {
                terminated = true;
                break;
            }
            if tag == 1 {
                if needs.len() >= 256 {
                    return Err(Error::Unsupported("too many DT_NEEDED entries"));
                }
                needs.push(value);
                continue;
            }
            // Loader-affecting unsupported features must never silently succeed.
            match tag {
                16 => return Err(Error::Unsupported("DT_SYMBOLIC")),
                17..=19 if value != 0 => return Err(Error::Unsupported("REL relocations")),
                22 => return Err(Error::Unsupported("DT_TEXTREL")),
                32 | 33 if value != 0 => return Err(Error::Unsupported("DT_PREINIT_ARRAY")),
                35..=37 if value != 0 => return Err(Error::Unsupported("RELR relocations")),
                0x6fff_fff0 | 0x6fff_fffc | 0x6fff_fffd | 0x6fff_fffe | 0x6fff_ffff
                    if value != 0 =>
                {
                    return Err(Error::Unsupported("symbol versioning"));
                }
                0x7fff_fffd | 0x7fff_ffff => {
                    return Err(Error::Unsupported("filter or auxiliary shared objects"));
                }
                0x6fff_fefc | 0x6fff_fefd => {
                    return Err(Error::Unsupported("dynamic audit modules"));
                }
                15 | 29 if value != 0 => {
                    return Err(Error::Unsupported(
                        "RPATH/RUNPATH (platform search paths only)",
                    ));
                }
                30 if value & !8 != 0 => {
                    return Err(Error::Unsupported("DT_FLAGS other than BIND_NOW"));
                }
                0x6fff_fffb if value & !(1 | 8 | 0x0800_0000) != 0 => {
                    return Err(Error::Unsupported("DT_FLAGS_1 mode"));
                }
                _ => {}
            }
            if tags.insert(tag, value).is_some() {
                return Err(Error::Format("duplicate dynamic tag"));
            }
        }
        if !terminated {
            return Err(Error::Format("unterminated dynamic table"));
        }
        let get = |tag: u64| tags.get(&tag).copied();
        let string_table = match (get(5), get(10)) {
            (Some(pointer), Some(size)) if size != 0 => {
                Some(range(bytes, self.file_offset(pointer, size)?, size)?)
            }
            (None, None) => None,
            _ => return Err(Error::Format("incomplete string table")),
        };
        if let Some(strings) = string_table {
            if strings[0] != 0 || *strings.last().unwrap() != 0 {
                return Err(Error::Format("invalid dynamic string table sentinels"));
            }
            for needed in needs {
                let name = string(strings, needed)?;
                if name.is_empty() {
                    return Err(Error::Format("empty DT_NEEDED name"));
                }
                self.needed.push(
                    core::str::from_utf8(name)
                        .map_err(|_| Error::Unsupported("non-UTF8 dependency name"))?
                        .to_string(),
                );
            }
            for tag in [14, 15, 29] {
                if let Some(index) = get(tag) {
                    string(strings, index)?;
                }
            }
        } else if !needs.is_empty() {
            return Err(Error::Format("DT_NEEDED without string table"));
        }
        if let Some(symtab) = get(6) {
            if get(11) != Some(24) {
                return Err(Error::Format("invalid or missing DT_SYMENT"));
            }
            let strings = string_table.ok_or(Error::Format("symbols without string table"))?;
            let max_symbols = (self.file_available(symtab)? / 24).min(MAX_ENTRIES);
            let mut count = None;
            if let Some(hash) = get(4) {
                count = Some(self.sysv_symbol_count(bytes, hash, max_symbols)?);
            }
            if let Some(hash) = get(0x6fff_fef5) {
                let gnu_count = self.gnu_symbol_count(bytes, hash, max_symbols)?;
                if count.is_some_and(|count| gnu_count > count) {
                    return Err(Error::Format("GNU hash exceeds SysV symbol count"));
                }
                if count.is_none() {
                    count = Some(gnu_count);
                }
            }
            let count = count.ok_or(Error::Unsupported(
                "dynamic symbol table requires SysV or GNU hash",
            ))?;
            if count == 0 {
                return Err(Error::Format("dynamic symbol table lacks null symbol"));
            }
            let table = self.file_offset(symtab, count.checked_mul(24).ok_or(Error::Overflow)?)?;
            if range(bytes, table, 24)?.iter().any(|b| *b != 0) {
                return Err(Error::Format("invalid null symbol"));
            }
            for i in 0..count {
                let offset = table + i * 24;
                let name = string(strings, u32_at(bytes, offset)? as usize)?.to_vec();
                let info = range(bytes, offset + 4, 1)?[0];
                let other = range(bytes, offset + 5, 1)?[0];
                let kind = info & 15;
                let binding = info >> 4;
                let shndx = u16_at(bytes, offset + 6)?;
                let value = address(bytes, offset + 8)?;
                let symbol_size = address(bytes, offset + 16)?;
                if kind == 6 {
                    return Err(Error::Unsupported("TLS symbol"));
                }
                if kind == 10 {
                    return Err(Error::Unsupported("GNU IFUNC symbol"));
                }
                if ![0, 1, 2, 3, 4].contains(&kind) {
                    return Err(Error::Unsupported("dynamic symbol type"));
                }
                if binding > 2 {
                    return Err(Error::Unsupported(
                        "dynamic symbol binding (including GNU_UNIQUE)",
                    ));
                }
                if other & !3 != 0 {
                    return Err(Error::Unsupported(
                        "architecture-specific symbol visibility",
                    ));
                }
                if shndx >= 0xff00 && shndx != 0xfff1 {
                    return Err(Error::Unsupported("reserved symbol section index"));
                }
                if shndx != 0
                    && shndx != 0xfff1
                    && !self.segments.iter().any(|s| s.contains(value, symbol_size))
                {
                    return Err(Error::Format("defined symbol is outside load segments"));
                }
                if shndx == 0 && other & 3 != 0 {
                    return Err(Error::Format("undefined non-default visibility symbol"));
                }
                if shndx != 0
                    && (binding == 1 || binding == 2)
                    && (other & 3 == 0 || other & 3 == 3)
                {
                    self.exports
                        .entry(name.clone())
                        .or_insert(self.symbols.len());
                }
                self.symbols.push(Symbol {
                    name,
                    value,
                    binding,
                    visibility: other & 3,
                    defined: shndx != 0,
                    absolute: shndx == 0xfff1,
                });
            }
        } else if get(4).is_some() || get(0x6fff_fef5).is_some() || get(11).is_some() {
            return Err(Error::Format("hash or symbol size without symbol table"));
        }
        let mut relocation_ranges = Vec::new();
        match (get(7), get(8)) {
            (Some(pointer), Some(size)) => {
                if get(9) != Some(24) {
                    return Err(Error::Format("invalid or missing DT_RELAENT"));
                }
                if size > 0 {
                    relocation_ranges.push((pointer, size));
                }
            }
            (None, None) => {}
            _ => return Err(Error::Format("incomplete RELA table")),
        }
        match (get(23), get(2)) {
            (Some(pointer), Some(size)) => {
                if get(20) != Some(7) {
                    return Err(Error::Unsupported("PLT relocations must use RELA"));
                }
                if size > 0 {
                    relocation_ranges.push((pointer, size));
                }
            }
            (None, None) => {}
            _ => return Err(Error::Format("incomplete PLT relocation table")),
        }
        if relocation_ranges.len() == 2 {
            let (a, asz) = relocation_ranges[0];
            let (b, bsz) = relocation_ranges[1];
            if a < b.checked_add(bsz).ok_or(Error::Overflow)?
                && b < a.checked_add(asz).ok_or(Error::Overflow)?
            {
                return Err(Error::Format("overlapping RELA and PLT tables"));
            }
        }
        for (pointer, size) in relocation_ranges {
            if size % 24 != 0 || size / 24 > MAX_ENTRIES {
                return Err(Error::Format("invalid RELA table size"));
            }
            let offset = self.file_offset(pointer, size)?;
            for i in 0..size / 24 {
                let row = offset + i * 24;
                let target = address(bytes, row)?;
                let info = u64_at(bytes, row + 8)?;
                let kind = info as u32;
                let symbol = (info >> 32) as usize;
                let addend = u64_at(bytes, row + 16)? as i64;
                let relative = match machine {
                    Machine::Aarch64 => kind == 1027,
                    Machine::Riscv64 => kind == 3,
                };
                let valid = match machine {
                    Machine::Aarch64 => [0, 257, 1025, 1026, 1027].contains(&kind),
                    Machine::Riscv64 => [0, 2, 3, 5].contains(&kind),
                };
                if !valid {
                    return Err(Error::Unsupported(
                        "relocation type (TLS, COPY, IFUNC and instruction relocations are unsupported)",
                    ));
                }
                if kind == 0 {
                    continue;
                }
                if relative && symbol != 0 {
                    return Err(Error::Format("RELATIVE relocation has a symbol"));
                }
                if !relative && symbol >= self.symbols.len() {
                    return Err(Error::Format("relocation symbol index is out of bounds"));
                }
                if !self
                    .segments
                    .iter()
                    .any(|s| s.permissions.write && !s.permissions.execute && s.contains(target, 8))
                {
                    return Err(Error::Unsupported(
                        "relocation target outside writable load segment",
                    ));
                }
                self.relocations.push(Relocation {
                    offset: target,
                    symbol,
                    kind,
                    addend,
                });
            }
        }
        if let Some(count) = get(0x6fff_fff9) {
            let main_count = get(8).unwrap_or(0) / 24;
            if count > main_count {
                return Err(Error::Format("DT_RELACOUNT exceeds RELA table"));
            }
            // RELACOUNT is only a hint; all relocation types remain checked.
        }
        if let Some(init) = get(12) {
            if init != 0
                && !self
                    .segments
                    .iter()
                    .any(|s| s.permissions.execute && s.contains(init, 1))
            {
                return Err(Error::Format("DT_INIT is outside executable segment"));
            }
            self.init = Some(init);
        }
        match (get(25), get(27)) {
            (Some(array), Some(size)) => {
                if size % 8 != 0 || size / 8 > MAX_ENTRIES {
                    return Err(Error::Format("invalid init array size"));
                }
                if size != 0
                    && !self
                        .segments
                        .iter()
                        .any(|s| s.permissions.read && s.contains(array, size))
                {
                    return Err(Error::Format("init array outside readable load segment"));
                }
                self.init_array = Some((array, size / 8));
            }
            (None, None) => {}
            _ => return Err(Error::Format("incomplete init array")),
        }
        Ok(())
    }
    fn sysv_symbol_count(
        &self,
        bytes: &[u8],
        pointer: usize,
        max_symbols: usize,
    ) -> Result<usize, Error> {
        let offset = self.file_offset(pointer, 8)?;
        let buckets = u32_at(bytes, offset)? as usize;
        let count = u32_at(bytes, offset + 4)? as usize;
        if buckets == 0 || buckets > MAX_ENTRIES || count > max_symbols {
            return Err(Error::Format("invalid SysV hash dimensions"));
        }
        let entries = buckets.checked_add(count).ok_or(Error::Overflow)?;
        self.file_offset(
            pointer,
            entries
                .checked_mul(4)
                .and_then(|n| n.checked_add(8))
                .ok_or(Error::Overflow)?,
        )?;
        for i in 0..entries {
            if u32_at(bytes, offset + 8 + i * 4)? as usize >= count {
                return Err(Error::Format("SysV hash symbol index out of bounds"));
            }
        }
        Ok(count)
    }
    fn gnu_symbol_count(
        &self,
        bytes: &[u8],
        pointer: usize,
        max_symbols: usize,
    ) -> Result<usize, Error> {
        let offset = self.file_offset(pointer, 16)?;
        let buckets = u32_at(bytes, offset)? as usize;
        let first = u32_at(bytes, offset + 4)? as usize;
        let bloom_size = u32_at(bytes, offset + 8)? as usize;
        let bloom_shift = u32_at(bytes, offset + 12)?;
        if buckets == 0
            || buckets > MAX_ENTRIES
            || bloom_size == 0
            || bloom_size > MAX_ENTRIES
            || !bloom_size.is_power_of_two()
            || bloom_shift >= 64
            || first > max_symbols
        {
            return Err(Error::Format("invalid GNU hash dimensions"));
        }
        let bucket_offset = 16usize
            .checked_add(bloom_size.checked_mul(8).ok_or(Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let chain_offset = bucket_offset
            .checked_add(buckets.checked_mul(4).ok_or(Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        self.file_offset(pointer, chain_offset)?;
        let available_chains = (self.file_available(pointer)? - chain_offset) / 4;
        let mut count = first;
        let mut visited = 0usize;
        for i in 0..buckets {
            let bucket = u32_at(bytes, offset + bucket_offset + i * 4)? as usize;
            if bucket == 0 {
                continue;
            }
            if bucket < first || bucket >= max_symbols {
                return Err(Error::Format("GNU hash bucket out of bounds"));
            }
            let mut symbol = bucket;
            loop {
                if symbol >= max_symbols || symbol - first >= available_chains {
                    return Err(Error::Format("unterminated GNU hash chain"));
                }
                visited += 1;
                if visited > MAX_ENTRIES {
                    return Err(Error::Format("GNU hash traversal exceeds limit"));
                }
                let hash = u32_at(bytes, offset + chain_offset + (symbol - first) * 4)?;
                count = count.max(symbol + 1);
                if hash & 1 != 0 {
                    break;
                }
                symbol += 1;
            }
        }
        Ok(count)
    }
}
fn range(bytes: &[u8], offset: usize, size: usize) -> Result<&[u8], Error> {
    bytes
        .get(offset..offset.checked_add(size).ok_or(Error::Overflow)?)
        .ok_or(Error::Format("file range out of bounds"))
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        range(bytes, offset, 2)?.try_into().unwrap(),
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        range(bytes, offset, 4)?.try_into().unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        range(bytes, offset, 8)?.try_into().unwrap(),
    ))
}
fn address(bytes: &[u8], offset: usize) -> Result<usize, Error> {
    usize::try_from(u64_at(bytes, offset)?).map_err(|_| Error::Overflow)
}
fn string(strings: &[u8], index: usize) -> Result<&[u8], Error> {
    let tail = strings
        .get(index..)
        .ok_or(Error::Format("string offset out of bounds"))?;
    let end = tail
        .iter()
        .position(|b| *b == 0)
        .ok_or(Error::Format("unterminated dynamic string"))?;
    Ok(&tail[..end])
}
fn down(address: usize) -> usize {
    address & !(PAGE_SIZE - 1)
}
fn up(address: usize) -> Result<usize, Error> {
    Ok(address.checked_add(PAGE_SIZE - 1).ok_or(Error::Overflow)? & !(PAGE_SIZE - 1))
}
