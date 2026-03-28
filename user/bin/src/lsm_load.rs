#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::env;
use std::fs::{File, list_directory};
use std::string::{String, ToString};
use std::syscall::{Syscall, syscall1, syscall2};
use std::vec::Vec;
use std::{format, println};

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;
const SHT_SYMTAB: u32 = 2;
const SHN_UNDEF: u16 = 0;
const ELF64_EHDR_SIZE: usize = 64;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;

const LSM_ERROR_MISSING_DEPENDENCY: usize = 10;
const LSM_LIST_ENTRY_SIZE: usize = 264;
const LSM_LIST_MAX_MODULES: usize = 128;

const MODULES_DIR: &str = "/scarlet/system/scarlet/modules";

fn read_u16(data: &[u8], offset: usize, little_endian: bool) -> Option<u16> {
    let bytes: [u8; 2] = data.get(offset..offset + 2)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(data: &[u8], offset: usize, little_endian: bool) -> Option<u32> {
    let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn read_u64(data: &[u8], offset: usize, little_endian: bool) -> Option<u64> {
    let bytes: [u8; 8] = data.get(offset..offset + 8)?.try_into().ok()?;
    Some(if little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

fn file_exists(path: &str) -> bool {
    File::open(path).is_ok()
}

fn read_file_bytes(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("failed to open {}", path))?;
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| format!("failed to read {}", path))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    Ok(data)
}

fn section_header_offset(shoff: usize, shentsize: usize, index: usize) -> Option<usize> {
    shoff.checked_add(shentsize.checked_mul(index)?)
}

fn find_symbol_data<'a>(data: &'a [u8], symbol_name: &str) -> Option<&'a [u8]> {
    if data.len() < ELF64_EHDR_SIZE {
        return None;
    }
    if data.get(0..4)? != [0x7F, b'E', b'L', b'F'] {
        return None;
    }
    if *data.get(4)? != ELFCLASS64 {
        return None;
    }

    let little_endian = match *data.get(5)? {
        ELFDATA2LSB => true,
        ELFDATA2MSB => false,
        _ => return None,
    };

    let shoff = usize::try_from(read_u64(data, 40, little_endian)?).ok()?;
    let shentsize = usize::from(read_u16(data, 58, little_endian)?);
    let shnum = usize::from(read_u16(data, 60, little_endian)?);

    if shentsize != ELF64_SHDR_SIZE {
        return None;
    }

    let section_table_end = shoff.checked_add(shentsize.checked_mul(shnum)?)?;
    if section_table_end > data.len() {
        return None;
    }

    let mut symtab_index = None;
    for idx in 0..shnum {
        let shdr_off = section_header_offset(shoff, shentsize, idx)?;
        let sh_type = read_u32(data, shdr_off + 4, little_endian)?;
        if sh_type == SHT_SYMTAB {
            symtab_index = Some(idx);
            break;
        }
    }
    let symtab_index = symtab_index?;
    let symtab_shdr_off = section_header_offset(shoff, shentsize, symtab_index)?;

    let symtab_offset =
        usize::try_from(read_u64(data, symtab_shdr_off + 24, little_endian)?).ok()?;
    let symtab_size = usize::try_from(read_u64(data, symtab_shdr_off + 32, little_endian)?).ok()?;
    let symtab_ent_size = {
        let raw = usize::try_from(read_u64(data, symtab_shdr_off + 56, little_endian)?).ok()?;
        if raw == 0 { ELF64_SYM_SIZE } else { raw }
    };
    if symtab_ent_size < ELF64_SYM_SIZE || symtab_size % symtab_ent_size != 0 {
        return None;
    }

    let strtab_index =
        usize::try_from(read_u32(data, symtab_shdr_off + 40, little_endian)?).ok()?;
    if strtab_index >= shnum {
        return None;
    }
    let strtab_shdr_off = section_header_offset(shoff, shentsize, strtab_index)?;
    let strtab_offset =
        usize::try_from(read_u64(data, strtab_shdr_off + 24, little_endian)?).ok()?;
    let strtab_size = usize::try_from(read_u64(data, strtab_shdr_off + 32, little_endian)?).ok()?;

    let strtab = data.get(strtab_offset..strtab_offset.checked_add(strtab_size)?)?;
    let symtab = data.get(symtab_offset..symtab_offset.checked_add(symtab_size)?)?;

    let symbol_count = symtab_size / symtab_ent_size;
    for idx in 0..symbol_count {
        let sym_off = idx * symtab_ent_size;
        let st_name = usize::try_from(read_u32(symtab, sym_off, little_endian)?).ok()?;
        let st_shndx = read_u16(symtab, sym_off + 6, little_endian)?;
        let st_value = usize::try_from(read_u64(symtab, sym_off + 8, little_endian)?).ok()?;

        if st_name >= strtab.len() {
            continue;
        }

        let mut end = st_name;
        while end < strtab.len() && strtab[end] != 0 {
            end += 1;
        }
        let name = core::str::from_utf8(&strtab[st_name..end]).ok()?;
        if name != symbol_name || st_shndx == SHN_UNDEF {
            continue;
        }

        let sec_index = usize::from(st_shndx);
        if sec_index >= shnum {
            return None;
        }
        let sec_shdr_off = section_header_offset(shoff, shentsize, sec_index)?;
        let sec_offset = usize::try_from(read_u64(data, sec_shdr_off + 24, little_endian)?).ok()?;
        let sec_size = usize::try_from(read_u64(data, sec_shdr_off + 32, little_endian)?).ok()?;

        if st_value >= sec_size {
            return None;
        }

        let data_offset = sec_offset.checked_add(st_value)?;
        let data_size = sec_size - st_value;
        return data.get(data_offset..data_offset.checked_add(data_size)?);
    }

    None
}

fn read_symbol_string(data: &[u8], symbol_name: &str, max_len: usize) -> Option<String> {
    let bytes = find_symbol_data(data, symbol_name)?;
    let mut len = 0usize;
    while len < bytes.len() && len < max_len && bytes[len] != 0 {
        len += 1;
    }
    core::str::from_utf8(&bytes[..len]).ok().map(String::from)
}

fn read_module_dependencies(data: &[u8]) -> Vec<String> {
    let deps = read_symbol_string(data, "SCARLET_LSM_DEPENDS", 1024).unwrap_or_default();
    deps.split(',')
        .map(str::trim)
        .filter(|dep| !dep.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn module_name_from_path(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if let Some(stem) = file_name.strip_suffix(".lsm") {
        return stem.to_string();
    }
    file_name.to_string()
}

fn read_module_name(data: &[u8], path: &str) -> String {
    read_symbol_string(data, "SCARLET_LSM_NAME", 256).unwrap_or_else(|| module_name_from_path(path))
}

fn parent_dir(path: &str) -> Option<&str> {
    let idx = path.rfind('/')?;
    if idx == 0 {
        Some("/")
    } else {
        Some(&path[..idx])
    }
}

fn list_loaded_module_names() -> Vec<String> {
    let mut buf = [0u8; LSM_LIST_ENTRY_SIZE * LSM_LIST_MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());
    if count == 0 {
        return Vec::new();
    }

    let mut names = Vec::new();
    let max_count = core::cmp::min(count, LSM_LIST_MAX_MODULES);
    for i in 0..max_count {
        let offset = i * LSM_LIST_ENTRY_SIZE;
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        if let Ok(name) = core::str::from_utf8(&name_bytes[..name_len])
            && !name.is_empty()
            && !names.iter().any(|existing| existing == name)
        {
            names.push(name.to_string());
        }
    }
    names
}

fn find_dependency_path(dep_name: &str, origin_path: &str) -> Option<String> {
    let mut candidates = Vec::new();

    candidates.push(format!("{}/{}.lsm", MODULES_DIR, dep_name));

    if let Some(dir) = parent_dir(origin_path) {
        candidates.push(format!("{}/{}.lsm", dir, dep_name));
    }

    for candidate in candidates {
        if file_exists(&candidate) {
            return Some(candidate);
        }
    }

    if let Ok(entries) = list_directory(MODULES_DIR) {
        for entry in entries {
            if !entry.is_file() {
                continue;
            }
            if entry.name == format!("{}.lsm", dep_name) {
                return Some(format!("{}/{}", MODULES_DIR, entry.name));
            }
        }
    }

    None
}

fn resolve_to_absolute(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    let filename = path.rsplit('/').next().unwrap_or(path);
    let abs = format!("{}/{}", MODULES_DIR, filename);
    if file_exists(&abs) {
        return abs;
    }
    path.to_string()
}

fn load_module_recursive(
    module_path: &str,
    visiting: &mut Vec<String>,
    loaded: &mut Vec<String>,
) -> Result<(), String> {
    let data = read_file_bytes(module_path)?;
    let module_name = read_module_name(&data, module_path);

    if loaded.iter().any(|name| name == &module_name) {
        return Ok(());
    }

    if visiting.iter().any(|name| name == &module_name) {
        return Err(format!("dependency cycle detected at {}", module_name));
    }

    visiting.push(module_name.clone());

    let dependencies = read_module_dependencies(&data);
    for dep_name in dependencies {
        if loaded.iter().any(|name| name == &dep_name) {
            continue;
        }

        let dep_path = match find_dependency_path(&dep_name, module_path) {
            Some(path) => path,
            None => {
                visiting.pop();
                return Err(format!(
                    "dependency {} not found for {}",
                    dep_name, module_name
                ));
            }
        };

        load_module_recursive(&dep_path, visiting, loaded)?;

        for loaded_name in list_loaded_module_names() {
            if !loaded.iter().any(|name| name == &loaded_name) {
                loaded.push(loaded_name);
            }
        }
    }

    let syscall_path = resolve_to_absolute(module_path);
    println!("loading module: {}", syscall_path);
    let ret = syscall1(Syscall::LsmLoad, syscall_path.as_ptr() as usize);
    visiting.pop();

    if ret == 0 {
        if !loaded.iter().any(|name| name == &module_name) {
            loaded.push(module_name);
        }
        return Ok(());
    }

    if ret == LSM_ERROR_MISSING_DEPENDENCY {
        return Err(format!(
            "failed to load {}: missing dependency (error {})",
            module_path, ret
        ));
    }

    Err(format!("failed to load {} (error: {})", module_path, ret))
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<std::string::String> = env::args().collect();

    if args.len() < 2 {
        println!("usage: lsm_load <module.lsm>");
        return 1;
    }

    let path_str = &args[1];
    let mut visiting = Vec::new();
    let mut loaded = list_loaded_module_names();

    match load_module_recursive(path_str, &mut visiting, &mut loaded) {
        Ok(()) => {
            println!("module loaded successfully");
            0
        }
        Err(e) => {
            println!("{}", e);
            1
        }
    }
}
