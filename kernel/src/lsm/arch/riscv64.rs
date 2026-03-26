use crate::lsm::elf::{
    ParsedSymbol, RelocObject, RelocationEntry, RelocationSection, SHN_UNDEF, STB_GLOBAL,
    STB_LOCAL, STB_WEAK, STT_SECTION,
};

pub const R_RISCV_NONE: u32 = 0;
pub const R_RISCV_32: u32 = 1;
pub const R_RISCV_64: u32 = 2;
pub const R_RISCV_BRANCH: u32 = 16;
pub const R_RISCV_JAL: u32 = 17;
pub const R_RISCV_CALL: u32 = 18;
pub const R_RISCV_CALL_PLT: u32 = 19;
pub const R_RISCV_PCREL_HI20: u32 = 23;
pub const R_RISCV_PCREL_LO12_I: u32 = 24;
pub const R_RISCV_PCREL_LO12_S: u32 = 25;
pub const R_RISCV_HI20: u32 = 26;
pub const R_RISCV_LO12_I: u32 = 27;
pub const R_RISCV_LO12_S: u32 = 28;
pub const R_RISCV_ADD32: u32 = 35;
pub const R_RISCV_SUB32: u32 = 39;
pub const R_RISCV_ALIGN: u32 = 43;
pub const R_RISCV_RVC_BRANCH: u32 = 44;
pub const R_RISCV_RVC_JUMP: u32 = 45;
pub const R_RISCV_RELAX: u32 = 51;
pub const R_RISCV_SET16: u32 = 55;
pub const R_RISCV_SET32: u32 = 56;
pub const R_RISCV_32_PCREL: u32 = 57;

pub fn apply_relocations(
    object: &RelocObject,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
) -> Result<(), &'static str> {
    for reloc_section in &object.relocation_sections {
        let target_section_index = reloc_section.target_section_index as usize;
        let target_base = section_base(section_bases, target_section_index)
            .ok_or("Missing base address for relocation target section")?;
        let target_section = object
            .sections
            .get(target_section_index)
            .ok_or("Relocation target section out of bounds")?;
        let target_size =
            usize::try_from(target_section.sh_size).map_err(|_| "Section size too large")?;

        for relocation in &reloc_section.entries {
            let location_addr =
                relocation_location(target_base, target_size, relocation.r_offset, 0)?;
            let place = location_addr as *mut u8;

            match relocation.r_type {
                R_RISCV_NONE | R_RISCV_RELAX => {}
                R_RISCV_32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)? as u32;
                    write_u32_le(place, value);
                }
                R_RISCV_64 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 8)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend)?)?;
                    write_u64_le(place, value);
                }
                R_RISCV_BRANCH => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let offset = s_plus_a_minus_p(s, relocation.r_addend, location_addr)?;
                    patch_b_type(place, offset)?;
                }
                R_RISCV_JAL => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let offset = s_plus_a_minus_p(s, relocation.r_addend, location_addr)?;
                    patch_j_type(place, offset)?;
                }
                R_RISCV_CALL | R_RISCV_CALL_PLT => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 8)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let offset = s_plus_a_minus_p(s, relocation.r_addend, location_addr)?;
                    patch_call_pair(place, offset)?;
                }
                R_RISCV_PCREL_HI20 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a_minus_p(s, relocation.r_addend, location_addr)?;
                    let hi = hi20(value);
                    patch_u_type(place, hi);
                }
                R_RISCV_PCREL_LO12_I => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let lo = paired_pcrel_lo12(
                        object,
                        reloc_section,
                        relocation,
                        section_bases,
                        symbol_resolver,
                        target_base,
                        target_size,
                    )?;
                    patch_i_type(place, lo as u32);
                }
                R_RISCV_PCREL_LO12_S => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let lo = paired_pcrel_lo12(
                        object,
                        reloc_section,
                        relocation,
                        section_bases,
                        symbol_resolver,
                        target_base,
                        target_size,
                    )?;
                    patch_s_type(place, lo as u32);
                }
                R_RISCV_HI20 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)?;
                    let hi = hi20(value);
                    patch_u_type(place, hi);
                }
                R_RISCV_LO12_I => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)?;
                    patch_i_type(place, value as u32);
                }
                R_RISCV_LO12_S => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)?;
                    patch_s_type(place, value as u32);
                }
                R_RISCV_ADD32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let addend = s_plus_a(s, relocation.r_addend)? as u32;
                    let cur = read_u32_le(place.cast_const());
                    write_u32_le(place, cur.wrapping_add(addend));
                }
                R_RISCV_SUB32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let sub = s_plus_a(s, relocation.r_addend)? as u32;
                    let cur = read_u32_le(place.cast_const());
                    write_u32_le(place, cur.wrapping_sub(sub));
                }
                R_RISCV_SET16 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 2)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)? as u16;
                    write_u16_le(place, value);
                }
                R_RISCV_SET32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend)? as u32;
                    write_u32_le(place, value);
                }
                R_RISCV_32_PCREL => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a_minus_p(s, relocation.r_addend, location_addr)? as u32;
                    write_u32_le(place, value);
                }
                R_RISCV_RVC_BRANCH | R_RISCV_RVC_JUMP => {
                    return Err("RVC not supported");
                }
                R_RISCV_ALIGN => {
                    return Err("R_RISCV_ALIGN not supported in kernel modules");
                }
                _ => return Err("Unsupported RISC-V relocation type"),
            }
        }
    }

    Ok(())
}

fn paired_pcrel_lo12(
    object: &RelocObject,
    reloc_section: &RelocationSection,
    lo_relocation: &RelocationEntry,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
    target_base: usize,
    target_size: usize,
) -> Result<i64, &'static str> {
    let lo_symbol = object
        .symbols
        .get(lo_relocation.r_sym as usize)
        .ok_or("Relocation symbol index out of bounds")?;
    let hi_offset = lo_symbol.value;

    let hi_relocation = reloc_section
        .entries
        .iter()
        .find(|entry| entry.r_type == R_RISCV_PCREL_HI20 && entry.r_offset == hi_offset)
        .ok_or("Missing paired R_RISCV_PCREL_HI20 relocation")?;

    section_write_ok(target_base, target_size, hi_relocation.r_offset, 4)?;
    let hi_location = relocation_location(target_base, target_size, hi_relocation.r_offset, 0)?;

    let hi_symbol_value =
        resolve_symbol_value(object, hi_relocation.r_sym, section_bases, symbol_resolver)?;
    let full_value = s_plus_a_minus_p(hi_symbol_value, hi_relocation.r_addend, hi_location)?;
    let hi = (hi20(full_value) as i64) << 12;
    Ok(full_value - hi)
}

fn resolve_symbol_value(
    object: &RelocObject,
    symbol_index: u32,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
) -> Result<usize, &'static str> {
    let symbol = object
        .symbols
        .get(symbol_index as usize)
        .ok_or("Relocation symbol index out of bounds")?;

    if symbol.typ == STT_SECTION {
        return resolve_section_symbol_base(symbol, section_bases);
    }

    if symbol.shndx != SHN_UNDEF {
        return resolve_defined_symbol(symbol, section_bases);
    }

    if symbol.bind == STB_GLOBAL || symbol.bind == STB_WEAK {
        return symbol_resolver(&symbol.name).ok_or("Undefined external symbol in relocation");
    }

    if symbol.bind == STB_LOCAL {
        return Err("Undefined local symbol in relocation");
    }

    Err("Unsupported symbol binding in relocation")
}

fn resolve_section_symbol_base(
    symbol: &ParsedSymbol,
    section_bases: &[(usize, usize)],
) -> Result<usize, &'static str> {
    section_base(section_bases, symbol.shndx as usize)
        .ok_or("Missing section base for STT_SECTION symbol")
}

fn resolve_defined_symbol(
    symbol: &ParsedSymbol,
    section_bases: &[(usize, usize)],
) -> Result<usize, &'static str> {
    let base = section_base(section_bases, symbol.shndx as usize)
        .ok_or("Missing section base for symbol")?;
    let value = usize::try_from(symbol.value).map_err(|_| "Symbol value too large")?;
    base.checked_add(value)
        .ok_or("Defined symbol address overflow")
}

fn section_base(section_bases: &[(usize, usize)], section_index: usize) -> Option<usize> {
    section_bases
        .iter()
        .find_map(|(idx, base)| (*idx == section_index).then_some(*base))
}

fn section_write_ok(
    section_base_addr: usize,
    section_size: usize,
    offset: u64,
    width: usize,
) -> Result<(), &'static str> {
    let off = usize::try_from(offset).map_err(|_| "Relocation offset too large")?;
    let end = off
        .checked_add(width)
        .ok_or("Relocation write range overflow")?;
    if end > section_size {
        return Err("Relocation write out of target section bounds");
    }
    let _ = section_base_addr;
    Ok(())
}

fn relocation_location(
    target_base: usize,
    target_size: usize,
    offset: u64,
    width: usize,
) -> Result<usize, &'static str> {
    section_write_ok(target_base, target_size, offset, width)?;
    let off = usize::try_from(offset).map_err(|_| "Relocation offset too large")?;
    target_base
        .checked_add(off)
        .ok_or("Relocation target address overflow")
}

fn s_plus_a(s: usize, addend: i64) -> Result<i64, &'static str> {
    let s_i64 = i64::try_from(s).map_err(|_| "Symbol value does not fit i64")?;
    s_i64
        .checked_add(addend)
        .ok_or("S + A overflow in relocation")
}

fn s_plus_a_minus_p(s: usize, addend: i64, p: usize) -> Result<i64, &'static str> {
    let sa = s_plus_a(s, addend)?;
    let p_i64 = i64::try_from(p).map_err(|_| "P value does not fit i64")?;
    sa.checked_sub(p_i64)
        .ok_or("S + A - P overflow in relocation")
}

fn to_u64(value: i64) -> Result<u64, &'static str> {
    u64::try_from(value).map_err(|_| "Relocation value is negative")
}

fn hi20(value: i64) -> u32 {
    ((value + 0x800) >> 12) as u32
}

fn patch_u_type(ptr: *mut u8, imm20: u32) {
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & 0x0000_0FFF) | ((imm20 & 0x000F_FFFF) << 12);
    write_u32_le(ptr, patched);
}

fn patch_i_type(ptr: *mut u8, imm: u32) {
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & 0x000F_FFFF) | ((imm & 0x0FFF) << 20);
    write_u32_le(ptr, patched);
}

fn patch_s_type(ptr: *mut u8, imm: u32) {
    let instruction = read_u32_le(ptr.cast_const());
    let imm12 = imm & 0x0FFF;
    let imm11_5 = (imm12 >> 5) & 0x7F;
    let imm4_0 = imm12 & 0x1F;
    let patched = (instruction & !((0x7F << 25) | (0x1F << 7))) | (imm11_5 << 25) | (imm4_0 << 7);
    write_u32_le(ptr, patched);
}

fn patch_b_type(ptr: *mut u8, offset: i64) -> Result<(), &'static str> {
    if (offset & 1) != 0 {
        return Err("R_RISCV_BRANCH offset is not 2-byte aligned");
    }
    if !fits_signed(offset, 13) {
        return Err("R_RISCV_BRANCH offset out of range");
    }

    let instruction = read_u32_le(ptr.cast_const());
    let imm = offset as i32 as u32;
    let bit12 = (imm >> 12) & 0x1;
    let bits10_5 = (imm >> 5) & 0x3F;
    let bits4_1 = (imm >> 1) & 0x0F;
    let bit11 = (imm >> 11) & 0x1;

    let patched = (instruction & !((0x7F << 25) | (0x1F << 7)))
        | (bit12 << 31)
        | (bits10_5 << 25)
        | (bits4_1 << 8)
        | (bit11 << 7);
    write_u32_le(ptr, patched);
    Ok(())
}

fn patch_j_type(ptr: *mut u8, offset: i64) -> Result<(), &'static str> {
    if (offset & 1) != 0 {
        return Err("R_RISCV_JAL offset is not 2-byte aligned");
    }
    if !fits_signed(offset, 21) {
        return Err("R_RISCV_JAL offset out of range");
    }

    let instruction = read_u32_le(ptr.cast_const());
    let imm = offset as i32 as u32;
    let bit20 = (imm >> 20) & 0x1;
    let bits10_1 = (imm >> 1) & 0x03FF;
    let bit11 = (imm >> 11) & 0x1;
    let bits19_12 = (imm >> 12) & 0xFF;

    let patched = (instruction & 0x0000_0FFF)
        | (bit20 << 31)
        | (bits10_1 << 21)
        | (bit11 << 20)
        | (bits19_12 << 12);
    write_u32_le(ptr, patched);
    Ok(())
}

fn patch_call_pair(ptr: *mut u8, offset: i64) -> Result<(), &'static str> {
    if !fits_signed(offset, 32) {
        return Err("R_RISCV_CALL offset out of range");
    }
    let hi = hi20(offset);
    let lo = (offset - ((hi as i64) << 12)) as u32;
    patch_u_type(ptr, hi);
    let jalr_ptr = ptr.wrapping_add(4);
    patch_i_type(jalr_ptr, lo);
    Ok(())
}

fn fits_signed(value: i64, bits: u32) -> bool {
    let shift = 64_u32.saturating_sub(bits);
    (value << shift >> shift) == value
}

fn read_u32_le(ptr: *const u8) -> u32 {
    let mut bytes = [0u8; 4];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), 4);
    }
    u32::from_le_bytes(bytes)
}

fn write_u32_le(ptr: *mut u8, val: u32) {
    let bytes = val.to_le_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, 4);
    }
}

fn write_u16_le(ptr: *mut u8, val: u16) {
    let bytes = val.to_le_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, 2);
    }
}

fn write_u64_le(ptr: *mut u8, val: u64) {
    let bytes = val.to_le_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, 8);
    }
}
