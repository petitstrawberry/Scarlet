use scarlet_sys::{Syscall, syscall1, syscall2};
use std::env;
use std::ffi::CString;
use std::fs;
use std::process::ExitCode;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;
const SHT_SYMTAB: u32 = 2;
const SHN_UNDEF: u16 = 0;
const ELF64_EHDR_SIZE: usize = 64;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;

const LSM_LIST_ENTRY_SIZE: usize = 264;
const LSM_LIST_MAX_MODULES: usize = 128;
const DEFAULT_MODULES_DIR: &str = "/scarlet/system/scarlet/modules";

fn main() -> ExitCode {
    println!("lsm_load: Rust std version");

    let args = env::args().collect::<Vec<_>>();
    if args.len() < 2 {
        println!("usage: lsm_load <module.lsm>");
        return ExitCode::from(1);
    }

    let mut visiting = Vec::new();
    let mut loaded = list_loaded_module_names();
    match load_module_recursive(&args[1], &mut visiting, &mut loaded) {
        Ok(()) => {
            println!("module loaded successfully");
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("{err}");
            ExitCode::from(1)
        }
    }
}

fn lsm_error_name(code: usize) -> &'static str {
    match code {
        0 => "success",
        1 => "invalid path",
        2 => "invalid ELF",
        3 => "out of memory",
        4 => "relocation error",
        5 => "no init symbol",
        6 => "init failed",
        7 => "build info mismatch",
        8 => "not found",
        9 => "permission denied",
        10 => "missing dependency",
        11 => "architecture mismatch",
        12 => "unresolved symbol",
        _ => "unknown error",
    }
}

fn modules_dirs() -> Vec<String> {
    let value = env::var("LSM_MODULES_PATH").unwrap_or_else(|_| DEFAULT_MODULES_DIR.to_string());
    let mut dirs = value
        .split(':')
        .filter(|dir| !dir.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if dirs.is_empty() {
        dirs.push(DEFAULT_MODULES_DIR.to_string());
    }
    dirs
}

fn read_u16(data: &[u8], offset: usize, little_endian: bool) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

fn read_u32(data: &[u8], offset: usize, little_endian: bool) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

fn read_u64(data: &[u8], offset: usize, little_endian: bool) -> Option<u64> {
    let bytes = data.get(offset..offset + 8)?.try_into().ok()?;
    Some(if little_endian {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    })
}

fn file_exists(path: &str) -> bool {
    fs::metadata(path).is_ok()
}

fn section_header_offset(shoff: usize, shentsize: usize, index: usize) -> Option<usize> {
    shoff.checked_add(shentsize.checked_mul(index)?)
}

fn find_symbol_data<'a>(data: &'a [u8], symbol_name: &str) -> Option<&'a [u8]> {
    if data.len() < ELF64_EHDR_SIZE || data.get(0..4)? != [0x7f, b'E', b'L', b'F'] {
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

    let symtab_shdr_off = section_header_offset(shoff, shentsize, symtab_index?)?;
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

    for idx in 0..(symtab_size / symtab_ent_size) {
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
        let name = std::str::from_utf8(&strtab[st_name..end]).ok()?;
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
    let len = bytes
        .iter()
        .take(max_len)
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len().min(max_len));
    std::str::from_utf8(&bytes[..len]).ok().map(String::from)
}

fn read_module_dependencies(data: &[u8]) -> Vec<String> {
    read_symbol_string(data, "SCARLET_LSM_DEPENDS", 1024)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|dep| !dep.is_empty())
        .map(str::to_string)
        .collect()
}

fn module_name_from_path(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name.strip_suffix(".lsm").unwrap_or(file_name).to_string()
}

fn read_module_name(data: &[u8], path: &str) -> String {
    read_symbol_string(data, "SCARLET_LSM_NAME", 256).unwrap_or_else(|| module_name_from_path(path))
}

fn parent_dir(path: &str) -> Option<&str> {
    let idx = path.rfind('/')?;
    if idx == 0 { Some("/") } else { Some(&path[..idx]) }
}

fn list_loaded_module_names() -> Vec<String> {
    let mut buf = [0; LSM_LIST_ENTRY_SIZE * LSM_LIST_MAX_MODULES];
    let count = syscall2(Syscall::LsmList, buf.as_mut_ptr() as usize, buf.len());
    if count == 0 {
        return Vec::new();
    }

    let mut names = Vec::new();
    for i in 0..count.min(LSM_LIST_MAX_MODULES) {
        let offset = i * LSM_LIST_ENTRY_SIZE;
        let name_bytes = &buf[offset + 8..offset + 8 + 256];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(256);
        if let Ok(name) = std::str::from_utf8(&name_bytes[..name_len])
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

    for dir in modules_dirs() {
        candidates.push(format!("{dir}/{dep_name}.lsm"));
    }

    if let Some(dir) = parent_dir(origin_path) {
        candidates.push(format!("{dir}/{dep_name}.lsm"));
    }

    for candidate in candidates {
        if file_exists(&candidate) {
            return Some(candidate);
        }
    }

    for dir in modules_dirs() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy() == format!("{dep_name}.lsm") {
                    return Some(format!("{dir}/{}", name.to_string_lossy()));
                }
            }
        }
    }

    None
}

fn resolve_to_absolute(path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    if file_exists(path) {
        let mut abs = String::from("/");
        abs.push_str(path);
        return abs;
    }

    let filename = path.rsplit('/').next().unwrap_or(path);
    for dir in modules_dirs() {
        let candidate = if filename.ends_with(".lsm") {
            format!("{dir}/{filename}")
        } else {
            format!("{dir}/{filename}.lsm")
        };
        if file_exists(&candidate) {
            return candidate;
        }
    }

    path.to_string()
}

fn load_module_recursive(
    module_path: &str,
    visiting: &mut Vec<String>,
    loaded: &mut Vec<String>,
) -> Result<(), String> {
    let resolved = resolve_to_absolute(module_path);
    let data = fs::read(&resolved).map_err(|err| format!("failed to read {resolved}: {err}"))?;
    let module_name = read_module_name(&data, module_path);

    if loaded.iter().any(|name| name == &module_name) {
        return Ok(());
    }
    if visiting.iter().any(|name| name == &module_name) {
        return Err(format!("dependency cycle detected at {module_name}"));
    }

    visiting.push(module_name.clone());

    for dep_name in read_module_dependencies(&data) {
        if loaded.iter().any(|name| name == &dep_name) {
            continue;
        }

        let dep_path = match find_dependency_path(&dep_name, module_path) {
            Some(path) => path,
            None => {
                visiting.pop();
                return Err(format!("dependency {dep_name} not found for {module_name}"));
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
    let c_syscall_path =
        CString::new(syscall_path.as_str()).map_err(|_| format!("invalid path {syscall_path}"))?;

    println!("loading module: {syscall_path}");
    let ret = syscall1(Syscall::LsmLoad, c_syscall_path.as_ptr() as usize);
    visiting.pop();

    if ret == 0 {
        if !loaded.iter().any(|name| name == &module_name) {
            loaded.push(module_name);
        }
        return Ok(());
    }

    Err(format!(
        "failed to load {module_path}: {} ({ret})",
        lsm_error_name(ret)
    ))
}
