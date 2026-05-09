use alloc::string::ToString;

use crate::lsm::RelocateError;
use crate::lsm::elf::{
    ParsedSymbol, RelocObject, RelocationEntry, RelocationSection, SHF_ALLOC, SHN_UNDEF,
    STB_GLOBAL, STB_LOCAL, STB_WEAK, STT_SECTION,
};

pub const MODULE_VA_START: usize = 0xffffffff81000000;
pub const MODULE_ELF_MACHINE: u16 = crate::lsm::elf::EM_AARCH64;

pub const R_AARCH64_NONE: u32 = 0;
pub const R_AARCH64_ABS64: u32 = 257;
pub const R_AARCH64_ABS32: u32 = 258;
pub const R_AARCH64_PREL32: u32 = 274;
pub const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;
pub const R_AARCH64_ADR_PREL_LO21: u32 = 276;
pub const R_AARCH64_ADD_ABS_LO12_NC: u32 = 277;
pub const R_AARCH64_LDST8_ABS_LO12_NC: u32 = 278;
pub const R_AARCH64_JUMP26: u32 = 282;
pub const R_AARCH64_CALL26: u32 = 283;
pub const R_AARCH64_MOVW_UABS_G0: u32 = 263;
pub const R_AARCH64_MOVW_UABS_G0_NC: u32 = 264;
pub const R_AARCH64_MOVW_UABS_G1_NC: u32 = 266;
pub const R_AARCH64_MOVW_UABS_G2_NC: u32 = 268;
pub const R_AARCH64_MOVW_UABS_G3: u32 = 269;

pub fn apply_relocations(
    object: &RelocObject,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
) -> Result<(), RelocateError> {
    let rerr = RelocateError::Relocation;
    for reloc_section in &object.relocation_sections {
        let target_section_index = reloc_section.target_section_index as usize;
        let target_section = match object.sections.get(target_section_index) {
            Some(s) => s,
            None => continue,
        };
        if (target_section.sh_flags & SHF_ALLOC) == 0 {
            continue;
        }
        let target_base = section_base(section_bases, target_section_index)
            .ok_or_else(|| rerr("Missing base address for relocation target section"))?;
        let target_size =
            usize::try_from(target_section.sh_size).map_err(|_| rerr("Section size too large"))?;

        for relocation in &reloc_section.entries {
            let location_addr =
                relocation_location(target_base, target_size, relocation.r_offset, 0)
                    .map_err(rerr)?;
            let place = location_addr as *mut u8;

            match relocation.r_type {
                R_AARCH64_NONE => {}

                R_AARCH64_ABS64 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 8)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    write_u64_le(place, value);
                }

                R_AARCH64_ABS32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value =
                        (s_plus_a(s, relocation.r_addend).map_err(rerr)? & 0xFFFF_FFFF) as u32;
                    write_u32_le(place, value);
                }

                R_AARCH64_PREL32 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a_minus_p(s, relocation.r_addend, location_addr)
                        .map_err(rerr)? as u32;
                    write_u32_le(place, value);
                }

                R_AARCH64_ADR_PREL_PG_HI21 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let result = s_plus_a(s, relocation.r_addend).map_err(rerr)?
                        - (location_addr as i64 & !(0xFFF as i64));
                    patch_adrp(place, result).map_err(rerr)?;
                }

                R_AARCH64_ADR_PREL_LO21 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let result =
                        s_plus_a_minus_p(s, relocation.r_addend, location_addr).map_err(rerr)?;
                    patch_adr(place, result).map_err(rerr)?;
                }

                R_AARCH64_ADD_ABS_LO12_NC => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend).map_err(rerr)? & 0xFFF;
                    patch_ldst_add_imm12(place, value);
                }

                R_AARCH64_LDST8_ABS_LO12_NC => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = s_plus_a(s, relocation.r_addend).map_err(rerr)? & 0xFFF;
                    patch_ldst_add_imm12(place, value);
                }

                R_AARCH64_JUMP26 | R_AARCH64_CALL26 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let offset =
                        s_plus_a_minus_p(s, relocation.r_addend, location_addr).map_err(rerr)?;
                    patch_b26(place, offset).map_err(rerr)?;
                }

                R_AARCH64_MOVW_UABS_G3 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    patch_movz_movk(place, ((value >> 48) & 0xFFFF) as u32);
                }

                R_AARCH64_MOVW_UABS_G2_NC => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    patch_movz_movk(place, ((value >> 32) & 0xFFFF) as u32);
                }

                R_AARCH64_MOVW_UABS_G1_NC => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    patch_movz_movk(place, ((value >> 16) & 0xFFFF) as u32);
                }

                R_AARCH64_MOVW_UABS_G0_NC => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    patch_movz_movk(place, ((value >> 0) & 0xFFFF) as u32);
                }

                R_AARCH64_MOVW_UABS_G0 => {
                    section_write_ok(target_base, target_size, relocation.r_offset, 4)
                        .map_err(rerr)?;
                    let s = resolve_symbol_value(
                        object,
                        relocation.r_sym,
                        section_bases,
                        symbol_resolver,
                    )?;
                    let value = to_u64(s_plus_a(s, relocation.r_addend).map_err(rerr)?);
                    patch_movz_movk(place, ((value >> 0) & 0xFFFF) as u32);
                }

                _ => return Err(rerr("Unsupported AArch64 relocation type")),
            }
        }
    }

    Ok(())
}

fn resolve_symbol_value(
    object: &RelocObject,
    symbol_index: u32,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
) -> Result<usize, RelocateError> {
    let symbol = object
        .symbols
        .get(symbol_index as usize)
        .ok_or_else(|| RelocateError::Relocation("Relocation symbol index out of bounds"))?;

    if symbol.typ == STT_SECTION {
        return resolve_section_symbol_base(symbol, section_bases)
            .map_err(|e| RelocateError::Relocation(e));
    }

    if symbol.shndx != SHN_UNDEF {
        return resolve_defined_symbol(symbol, section_bases)
            .map_err(|e| RelocateError::Relocation(e));
    }

    if symbol.bind == STB_GLOBAL || symbol.bind == STB_WEAK {
        return symbol_resolver(&symbol.name)
            .ok_or_else(|| RelocateError::UnresolvedSymbol(symbol.name.to_string()));
    }

    if symbol.bind == STB_LOCAL {
        return Err(RelocateError::Relocation(
            "Undefined local symbol in relocation",
        ));
    }

    Err(RelocateError::Relocation(
        "Unsupported symbol binding in relocation",
    ))
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
    let s_i64 = s as u64 as i64;
    s_i64
        .checked_add(addend)
        .ok_or("S + A overflow in relocation")
}

fn s_plus_a_minus_p(s: usize, addend: i64, p: usize) -> Result<i64, &'static str> {
    let sa = s_plus_a(s, addend)?;
    let p_i64 = p as u64 as i64;
    sa.checked_sub(p_i64)
        .ok_or("S + A - P overflow in relocation")
}

fn to_u64(value: i64) -> u64 {
    value as u64
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

fn write_u64_le(ptr: *mut u8, val: u64) {
    let bytes = val.to_le_bytes();
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, 8);
    }
}

/// Patch ADRP instruction with a page-offset value.
///
/// ADRP encodes: result = page(S + A) - page(P)
/// The 21-bit signed immediate is split across two bit fields:
///   - immlo: bits [1:0] of (result >> 12)
///   - immhi: bits [20:2] of (result >> 12)
fn patch_adrp(ptr: *mut u8, page_offset: i64) -> Result<(), &'static str> {
    let value = page_offset >> 12;
    if !fits_signed(value, 21) {
        return Err("ADRP page offset out of range");
    }
    let immlo = (value as u32) & 0x3;
    let immhi = ((value as u32) >> 2) & 0x7FFFF;
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & !((0x3 << 29) | (0x7FFFF << 5))) | (immlo << 29) | (immhi << 5);
    write_u32_le(ptr, patched);
    Ok(())
}

/// Patch ADR instruction with a PC-relative value.
///
/// ADR encodes: result = (S + A) - P
/// Same encoding as ADRP but the value is not page-aligned.
fn patch_adr(ptr: *mut u8, offset: i64) -> Result<(), &'static str> {
    if !fits_signed(offset, 21) {
        return Err("ADR offset out of range");
    }
    let immlo = (offset as u32) & 0x3;
    let immhi = ((offset as u32) >> 2) & 0x7FFFF;
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & !((0x3 << 29) | (0x7FFFF << 5))) | (immlo << 29) | (immhi << 5);
    write_u32_le(ptr, patched);
    Ok(())
}

/// Patch the 12-bit immediate field of LDR/STR/ADD (unsigned offset) instructions.
///
/// Bits [21:10] of the instruction = imm12.
fn patch_ldst_add_imm12(ptr: *mut u8, imm12: i64) {
    let imm = (imm12 as u32) & 0xFFF;
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & !(0xFFF << 10)) | (imm << 10);
    write_u32_le(ptr, patched);
}

/// Patch B/BL instruction with a 26-bit signed offset.
///
/// Bits [31:26] = imm26 where imm26 = offset >> 2.
fn patch_b26(ptr: *mut u8, offset: i64) -> Result<(), &'static str> {
    if offset & 1 != 0 {
        return Err("B/BL offset is not 2-byte aligned");
    }
    if !fits_signed(offset, 28) {
        return Err("B/BL offset out of range (±128MB)");
    }
    let imm26 = ((offset >> 2) as u32) & 0x3FFFFFF;
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & !0x3FFFFFF) | imm26;
    write_u32_le(ptr, patched);
    Ok(())
}

/// Patch MOVZ or MOVK instruction with a 16-bit immediate.
///
/// MOVZ: load zero with 16-bit immediate at given shift
/// MOVK: keep bits and OR in 16-bit immediate at given shift
/// Bits [21:5] = imm16, bits [20:5] are shared between MOVZ and MOVK.
fn patch_movz_movk(ptr: *mut u8, imm16: u32) {
    let instruction = read_u32_le(ptr.cast_const());
    let patched = (instruction & !(0xFFFF << 5)) | ((imm16 & 0xFFFF) << 5);
    write_u32_le(ptr, patched);
}
