use clap::Parser;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::mem::size_of;
use std::path::PathBuf;

/// Darwin syscall table generator for Scarlet OS
///
/// Extracts BSD syscall numbers and Mach trap numbers from XNU sources
/// or libSystem_kernel.dylib and generates syscall_table.rs.
#[derive(Parser, Debug)]
#[command(name = "darwin-syscall-gen")]
#[command(about = "Generate Darwin syscall tables for Scarlet OS")]
struct Args {
    /// Path to XNU bsd/kern/syscalls.master file
    #[arg(long, required = true)]
    syscalls_master: PathBuf,

    /// Path to XNU osfmk/kern/syscall_sw.c file
    #[arg(long, required = true)]
    mach_traps: PathBuf,

    /// Optional: Path to libSystem_kernel.dylib for verification
    #[arg(long)]
    dylib: Option<PathBuf>,

    /// Output file path
    #[arg(short, long, default_value = "kernel/src/abi/darwin/aarch64/syscall_table.rs")]
    output: PathBuf,

    /// macOS version string for header comment
    #[arg(short, long, default_value = "unknown")]
    version: String,
}

/// A BSD syscall entry
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BsdSyscall {
    number: u32,
    name: String,
    nargs: u8,
    audit_event: String,
    compat: String,
}

/// A Mach trap entry
#[derive(Debug, Clone)]
struct MachTrap {
    number: i32,
    name: String,
    nargs: u8,
}

const MH_MAGIC_64: u32 = 0xFEEDFACF;
const MH_DYLIB: u32 = 0x06;
const CPU_TYPE_ARM64: u32 = 0x0100000C;
#[allow(dead_code)]
const CPU_SUBTYPE_ALL: u32 = 0x00000000;

const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x02;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct MachHeader64 {
    magic: u32,
    cputype: u32,
    cpusubtype: u32,
    filetype: u32,
    ncmds: u32,
    sizeofcmds: u32,
    flags: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct LoadCommand {
    cmd: u32,
    cmdsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct SegmentCommand64 {
    cmd: u32,
    cmdsize: u32,
    segname: [u8; 16],
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
    maxprot: i32,
    initprot: i32,
    nsects: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Section64 {
    sectname: [u8; 16],
    segname: [u8; 16],
    addr: u64,
    size: u64,
    offset: u32,
    align: u32,
    reloff: u32,
    nreloc: u32,
    flags: u32,
    reserved1: u32,
    reserved2: u32,
    reserved3: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct SymtabCommand {
    cmd: u32,
    cmdsize: u32,
    symoff: u32,
    nsyms: u32,
    stroff: u32,
    strsize: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Nlist64 {
    n_strx: u32,
    n_type: u8,
    n_sect: u8,
    n_desc: u16,
    n_value: u64,
}

fn decode_mov_imm16(instruction: u32) -> Option<u16> {
    if (instruction >> 23) & 0b111111 != 0b100101 {
        return None;
    }
    let opc = (instruction >> 29) & 0b11;
    if opc != 0b10 && opc != 0b11 {
        return None;
    }
    let imm16 = (instruction >> 5) & 0xFFFF;
    Some(imm16 as u16)
}

fn decode_mov_hw(instruction: u32) -> Option<u32> {
    if (instruction >> 23) & 0b111111 != 0b100101 {
        return None;
    }
    let hw = (instruction >> 21) & 0b11;
    Some(hw * 16)
}

fn is_movz(instruction: u32) -> bool {
    (instruction >> 23) & 0b111111 == 0b100101 && ((instruction >> 29) & 0b11) == 0b10
}

fn is_movk(instruction: u32) -> bool {
    (instruction >> 23) & 0b111111 == 0b100101 && ((instruction >> 29) & 0b11) == 0b11
}

fn is_svc(instruction: u32) -> bool {
    (instruction & 0xFFC0001F) == 0xD4000001
}

fn extract_syscall_number(instructions: &[u32]) -> Option<u32> {
    if instructions.len() < 3 {
        return None;
    }

    let mut syscall_num: u32 = 0;
    let mut found_movz = false;
    let mut found_movk = false;

    for &inst in instructions {
        if is_movz(inst) {
            let imm16 = decode_mov_imm16(inst)? as u32;
            let hw = decode_mov_hw(inst)?;
            syscall_num |= imm16 << hw;
            found_movz = true;
        } else if is_movk(inst) {
            let imm16 = decode_mov_imm16(inst)? as u32;
            let hw = decode_mov_hw(inst)?;
            syscall_num |= imm16 << hw;
            found_movk = true;
        } else if is_svc(inst)
            && found_movz {
                return Some(syscall_num);
            }
    }

    if found_movz && !found_movk {
        return Some(syscall_num);
    }

    None
}

fn read_struct<T: Copy>(bytes: &[u8]) -> Result<T, String> {
    if bytes.len() < size_of::<T>() {
        return Err("Truncated Mach-O structure".to_string());
    }
    Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr() as *const T) })
}

#[allow(dead_code)]
fn read_u32(bytes: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&bytes[..4]);
    u32::from_le_bytes(buf)
}

#[allow(dead_code)]
fn read_u64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(buf)
}

fn segname_to_string(segname: &[u8]) -> String {
    let len = segname.iter().position(|&b| b == 0).unwrap_or(segname.len());
    String::from_utf8_lossy(&segname[..len]).to_string()
}

struct DylibInfo {
    text_section_data: Vec<u8>,
    text_section_addr: u64,
    text_section_fileoff: u64,
    symbols: Vec<(String, u64)>,
}

fn parse_dylib(path: &PathBuf) -> Result<DylibInfo, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed to open dylib: {}", e))?;

    let mut header_bytes = [0u8; size_of::<MachHeader64>()];
    file.read_exact(&mut header_bytes)
        .map_err(|e| format!("Failed to read Mach-O header: {}", e))?;
    let header = read_struct::<MachHeader64>(&header_bytes)?;

    if header.magic != MH_MAGIC_64 {
        return Err(format!("Invalid Mach-O magic: 0x{:08X}", header.magic));
    }
    if header.cputype != CPU_TYPE_ARM64 {
        return Err(format!("Unsupported CPU type: 0x{:08X}", header.cputype));
    }
    if header.filetype != MH_DYLIB {
        return Err(format!("Not a dylib filetype: 0x{:08X}", header.filetype));
    }

    let mut load_commands = vec![0u8; header.sizeofcmds as usize];
    file.read_exact(&mut load_commands)
        .map_err(|e| format!("Failed to read load commands: {}", e))?;

    let mut text_section_data = Vec::new();
    let mut text_section_addr = 0u64;
    let mut text_section_fileoff = 0u64;
    let mut symtab_cmd: Option<SymtabCommand> = None;

    let mut offset = 0usize;
    for _ in 0..header.ncmds {
        let cmd_end = offset + size_of::<LoadCommand>();
        if cmd_end > load_commands.len() {
            return Err("Load command table truncated".to_string());
        }

        let load_cmd = read_struct::<LoadCommand>(&load_commands[offset..cmd_end])?;
        let cmdsize = load_cmd.cmdsize as usize;
        if cmdsize < size_of::<LoadCommand>() {
            return Err("Invalid load command size".to_string());
        }

        let next_offset = offset + cmdsize;
        if next_offset > load_commands.len() {
            return Err("Load command exceeds command table".to_string());
        }

        let command_bytes = &load_commands[offset..next_offset];

        match load_cmd.cmd {
            LC_SEGMENT_64 => {
                if cmdsize < size_of::<SegmentCommand64>() {
                    return Err("Truncated LC_SEGMENT_64".to_string());
                }
                let seg = read_struct::<SegmentCommand64>(
                    &command_bytes[..size_of::<SegmentCommand64>()],
                )?;
                let segname = segname_to_string(&seg.segname);

                if segname == "__TEXT" {
                    let mut sect_offset = size_of::<SegmentCommand64>();
                    for _ in 0..seg.nsects {
                        let sect_end = sect_offset + size_of::<Section64>();
                        if sect_end > cmdsize {
                            return Err("Section data exceeds segment command".to_string());
                        }
                        let section = read_struct::<Section64>(
                            &command_bytes[sect_offset..sect_end],
                        )?;
                        let sectname = segname_to_string(&section.sectname);

                        if sectname == "__text" {
                            text_section_addr = section.addr;
                            text_section_fileoff = section.offset as u64;
                            let sect_size = section.size as usize;

                            file.seek(SeekFrom::Start(text_section_fileoff))
                                .map_err(|e| format!("Failed to seek to __text: {}", e))?;
                            text_section_data.resize(sect_size, 0);
                            file.read_exact(&mut text_section_data)
                                .map_err(|e| format!("Failed to read __text: {}", e))?;
                        }

                        sect_offset = sect_end;
                    }
                }
            }
            LC_SYMTAB => {
                if cmdsize >= size_of::<SymtabCommand>() {
                    symtab_cmd = Some(read_struct::<SymtabCommand>(
                        &command_bytes[..size_of::<SymtabCommand>()],
                    )?);
                }
            }
            _ => {}
        }

        offset = next_offset;
    }

    let mut symbols = Vec::new();
    if let Some(symtab) = symtab_cmd {
        let mut strtab = vec![0u8; symtab.strsize as usize];
        file.seek(SeekFrom::Start(symtab.stroff as u64))
            .map_err(|e| format!("Failed to seek to string table: {}", e))?;
        file.read_exact(&mut strtab)
            .map_err(|e| format!("Failed to read string table: {}", e))?;

        let mut sym_data = vec![0u8; (symtab.nsyms as usize) * size_of::<Nlist64>()];
        file.seek(SeekFrom::Start(symtab.symoff as u64))
            .map_err(|e| format!("Failed to seek to symbol table: {}", e))?;
        file.read_exact(&mut sym_data)
            .map_err(|e| format!("Failed to read symbol table: {}", e))?;

        for i in 0..symtab.nsyms as usize {
            let sym_start = i * size_of::<Nlist64>();
            let sym_end = sym_start + size_of::<Nlist64>();
            let sym = read_struct::<Nlist64>(&sym_data[sym_start..sym_end])?;

            if sym.n_type & 0x0E == 0x0E {
                let str_offset = sym.n_strx as usize;
                if str_offset < strtab.len() {
                    let name_end = strtab[str_offset..]
                        .iter()
                        .position(|&b| b == 0)
                        .unwrap_or(strtab.len() - str_offset);
                    let name = String::from_utf8_lossy(&strtab[str_offset..str_offset + name_end])
                        .to_string();
                    if !name.is_empty() {
                        symbols.push((name, sym.n_value));
                    }
                }
            }
        }
    }

    Ok(DylibInfo {
        text_section_data,
        text_section_addr,
        text_section_fileoff,
        symbols,
    })
}

fn main() {
    let args = Args::parse();

    println!("darwin-syscall-gen: Generating Darwin syscall tables...");
    println!("  syscalls.master: {}", args.syscalls_master.display());
    println!("  mach_traps: {}", args.mach_traps.display());
    if let Some(ref dylib) = args.dylib {
        println!("  dylib: {}", dylib.display());
    }
    println!("  output: {}", args.output.display());

    let bsd_syscalls = parse_syscalls_master(&args.syscalls_master)
        .unwrap_or_else(|e| {
            eprintln!("Error parsing syscalls.master: {}", e);
            std::process::exit(1);
        });

    let mach_traps = parse_mach_traps(&args.mach_traps)
        .unwrap_or_else(|e| {
            eprintln!("Error parsing mach traps: {}", e);
            std::process::exit(1);
        });

    // If dylib provided, verify syscall numbers match
    if let Some(ref dylib_path) = args.dylib {
        println!("Verifying against libSystem_kernel.dylib...");
        match verify_against_dylib(dylib_path, &bsd_syscalls) {
            Ok(mismatches) => {
                if mismatches.is_empty() {
                    println!("  All syscall numbers verified against dylib!");
                } else {
                    println!("  {} mismatches found:", mismatches.len());
                    for (name, expected, actual) in &mismatches {
                        println!("    {}: expected={}, dylib={}", name, expected, actual);
                    }
                }
            }
            Err(e) => {
                eprintln!("  Warning: dylib verification failed: {}", e);
            }
        }
    }

    let generated = generate_syscall_table(&args.version, &bsd_syscalls, &mach_traps);

    fs::write(&args.output, generated)
        .unwrap_or_else(|e| {
            eprintln!("Error writing output: {}", e);
            std::process::exit(1);
        });

    println!(
        "Generated {} with {} BSD syscalls and {} Mach traps",
        args.output.display(),
        bsd_syscalls.len(),
        mach_traps.len()
    );
}

/// Parse XNU syscalls.master file
///
/// Format per line:
///   number AUE_EVENT COMPAT { ret_type name(args); } [attributes]
///
/// Example:
///   1 AUE_EXIT ALL { void exit(int rval); }
fn parse_syscalls_master(path: &PathBuf) -> Result<Vec<BsdSyscall>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let mut syscalls = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        // Skip preprocessor directives and C-style comments
        if line.starts_with("#if") || line.starts_with("#else") || line.starts_with("#endif") {
            continue;
        }

        match parse_syscalls_master_line(line) {
            Some(syscall) => syscalls.push(syscall),
            None => {
                // Some lines might be continuations or alternate entries - skip silently
                // unless it's clearly a malformed syscall line
                if line.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                    eprintln!("  Warning: Could not parse line {}: {}", line_num + 1, line);
                }
            }
        }
    }

    Ok(syscalls)
}

/// Parse a single syscalls.master line
fn parse_syscalls_master_line(line: &str) -> Option<BsdSyscall> {
    // Split by whitespace to get fields
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }

    // First field: syscall number
    let number: u32 = parts[0].parse().ok()?;

    // Second field: audit event (e.g., AUE_NULL, AUE_EXIT)
    let audit_event = parts[1].to_string();

    // Third field: compatibility flag (e.g., ALL, NOSTD, NOT_IMPLEMENTED)
    let compat = parts[2].to_string();

    // Find the prototype in braces: { ret_type name(args); }
    let brace_start = line.find('{')?;
    let brace_end = line.rfind('}')?;
    let prototype = &line[brace_start + 1..brace_end];

    // Parse prototype: "ret_type name(args)" or "ret_type name(args);"
    let prototype = prototype.trim().trim_end_matches(';');

    // Extract function name and count arguments
    let (name, nargs) = parse_prototype(prototype)?;

    Some(BsdSyscall {
        number,
        name,
        nargs,
        audit_event,
        compat,
    })
}

/// Parse a C function prototype to extract name and argument count
///
/// Examples:
///   "void exit(int rval)" -> ("exit", 1)
///   "pid_t fork(void)" -> ("fork", 0)
///   "ssize_t read(int fd, user_addr_t cbuf, user_size_t nbyte)" -> ("read", 3)
fn parse_prototype(proto: &str) -> Option<(String, u8)> {
    // Remove "NO_FP" prefix if present
    let proto = proto.trim_start_matches("NO_FP").trim();

    // Find the opening paren for arguments
    let paren_start = proto.find('(')?;
    let paren_end = proto.rfind(')')?;

    let before_parens = &proto[..paren_start].trim();
    let args_str = &proto[paren_start + 1..paren_end].trim();

    // Extract function name: it's the last word before the parens
    let name = before_parens
        .split_whitespace()
        .last()
        .map(|s| s.to_string())?;

    // Count arguments
    let nargs = if *args_str == "void" || args_str.is_empty() {
        0
    } else {
        // Count commas and add 1, but handle nested parens/angles
        let mut count = 1u8;
        let mut depth_paren = 0;
        let mut depth_angle = 0;

        for c in args_str.chars() {
            match c {
                '(' => depth_paren += 1,
                ')' => depth_paren -= 1,
                '<' => depth_angle += 1,
                '>' => depth_angle -= 1,
                ',' if depth_paren == 0 && depth_angle == 0 => count += 1,
                _ => {}
            }
        }

        count
    };

    Some((name, nargs))
}

/// Parse XNU osfmk/kern/syscall_sw.c for Mach traps
///
/// Looks for MACH_TRAP entries in the mach_trap_table array.
fn parse_mach_traps(path: &PathBuf) -> Result<Vec<MachTrap>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let mut traps = Vec::new();
    let mut in_table = false;
    let mut trap_number: i32 = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // Look for table start
        if trimmed.contains("mach_trap_table[") {
            in_table = true;
            continue;
        }

        if !in_table {
            continue;
        }

        // Table end
        if trimmed == "};" {
            break;
        }

        if trimmed.contains("MACH_TRAP(") {
            let num = if let Some(start) = trimmed.find("/*") {
                if let Some(end) = trimmed.find("*/") {
                    trimmed[start + 2..end].trim().parse::<u32>().ok()
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(num) = num {
                trap_number = -(num as i32);
            }

            if let Some(start) = trimmed.find("MACH_TRAP(") {
                let trap_line = &trimmed[start..];
                if let Some(trap) = parse_mach_trap_line(trap_line, trap_number) {
                    traps.push(trap);
                }
            }
        }
    }

    Ok(traps)
}

fn parse_mach_trap_line(line: &str, number: i32) -> Option<MachTrap> {
    let start = line.find('(')?;
    let end = line.rfind(')')?;
    let inner = &line[start + 1..end];

    let parts: Vec<&str> = inner.split(',').collect();
    if parts.len() < 2 {
        return None;
    }

    let name = parts[0].trim().to_string();
    let nargs: u8 = parts[1].trim().parse().ok()?;

    Some(MachTrap {
        number,
        name,
        nargs,
    })
}

fn verify_against_dylib(
    dylib_path: &PathBuf,
    syscalls: &[BsdSyscall],
) -> Result<Vec<(String, u32, u32)>, String> {
    let dylib = parse_dylib(dylib_path)?;

    if dylib.text_section_data.is_empty() {
        return Err("No __text section found in dylib".to_string());
    }

    let mut mismatches = Vec::new();

    for syscall in syscalls {
        let wrapper_name = format!("___{}_nocancel", syscall.name);

        let symbol_addr = dylib
            .symbols
            .iter()
            .find(|(name, _)| name == &wrapper_name || name == &syscall.name)
            .map(|(_, addr)| *addr);

        if let Some(addr) = symbol_addr {
            let file_offset = addr - dylib.text_section_addr + dylib.text_section_fileoff;
            let offset_in_section = (file_offset - dylib.text_section_fileoff) as usize;

            if offset_in_section + 16 <= dylib.text_section_data.len() {
                let inst_bytes = &dylib.text_section_data[offset_in_section..offset_in_section + 16];
                let instructions: Vec<u32> = inst_bytes
                    .chunks_exact(4)
                    .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();

                if let Some(dylib_syscall_num) = extract_syscall_number(&instructions) {
                    let expected = syscall.number;
                    if dylib_syscall_num != expected {
                        mismatches.push((
                            syscall.name.clone(),
                            expected,
                            dylib_syscall_num,
                        ));
                    }
                }
            }
        }
    }

    Ok(mismatches)
}

/// Generate the syscall_table.rs file content
fn generate_syscall_table(
    version: &str,
    bsd_syscalls: &[BsdSyscall],
    mach_traps: &[MachTrap],
) -> String {
    let mut output = String::new();

    // Header
    output.push_str("//! Darwin syscall and Mach trap tables for AArch64\n");
    output.push_str("//!\n");
    output.push_str("//! Auto-generated by darwin_syscall_gen\n");
    output.push_str(&format!("//! macOS version: {}\n", version));
    output.push_str("//!\n");
    output.push_str("//! DO NOT EDIT MANUALLY\n");
    output.push_str("//!\n");
    output.push_str("//! BSD syscall numbers use SYSCALL_CLASS_UNIX offset (0x2000000).\n");
    output.push_str("//! Mach trap numbers are negative (sign-extended).\n");
    output.push_str("//!\n");
    output.push_str("//! Sources:\n");
    output.push_str("//!   - XNU bsd/kern/syscalls.master\n");
    output.push_str("//!   - XNU osfmk/kern/syscall_sw.c\n");
    output.push('\n');

    // Version constant
    output.push_str(&format!("pub const SYSCALL_TABLE_VERSION: &str = \"{}\";\n", version));
    output.push('\n');

    // Syscall class
    output.push_str("pub const SYSCALL_CLASS_UNIX: u32 = 0x2000000;\n");
    output.push('\n');

    // BSD syscall constants
    output.push_str("// BSD syscall numbers (raw, without class offset)\n");
    let mut seen_const_names = std::collections::HashSet::new();
    for syscall in bsd_syscalls {
        let const_name = sanitize_syscall_name(&syscall.name);
        if !seen_const_names.insert(const_name.clone()) {
            continue;
        }
        output.push_str(&format!(
            "pub const SYS_{}: u32 = {};\n",
            const_name, syscall.number
        ));
    }
    output.push('\n');

    // Mach trap constants
    output.push_str("// Mach trap numbers (negative)\n");
    let mut seen_mach_names = std::collections::HashSet::new();
    for trap in mach_traps {
        let const_name = sanitize_mach_trap_name(&trap.name);
        if !seen_mach_names.insert(const_name.clone()) {
            continue;
        }
        output.push_str(&format!(
            "pub const MACH_{}: i32 = {};\n",
            const_name, trap.number
        ));
    }
    output.push('\n');

    // BSD syscall table struct and array
    output.push_str("/// BSD syscall table entry\n");
    output.push_str("pub struct BsdSyscallEntry {\n");
    output.push_str("    pub number: u32,\n");
    output.push_str("    pub name: &\'static str,\n");
    output.push_str("    pub nargs: u8,\n");
    output.push_str("    pub implemented: bool,\n");
    output.push_str("}\n");
    output.push('\n');

    output.push_str("pub const BSD_SYSCALL_TABLE: &[BsdSyscallEntry] = &[\n");
    for syscall in bsd_syscalls {
        output.push_str(&format!(
            "    BsdSyscallEntry {{ number: {}, name: \"{}\", nargs: {}, implemented: false }},\n",
            syscall.number, syscall.name, syscall.nargs
        ));
    }
    output.push_str("];\n");
    output.push('\n');

    // Mach trap table struct and array
    output.push_str("/// Mach trap table entry\n");
    output.push_str("pub struct MachTrapEntry {\n");
    output.push_str("    pub number: i32,\n");
    output.push_str("    pub name: &\'static str,\n");
    output.push_str("    pub nargs: u8,\n");
    output.push_str("    pub implemented: bool,\n");
    output.push_str("}\n");
    output.push('\n');

    output.push_str("pub const MACH_TRAP_TABLE: &[MachTrapEntry] = &[\n");
    for trap in mach_traps {
        output.push_str(&format!(
            "    MachTrapEntry {{ number: {}, name: \"{}\", nargs: {}, implemented: false }},\n",
            trap.number, trap.name, trap.nargs
        ));
    }
    output.push_str("];\n");
    output.push('\n');

    // Utility function: get syscall name by number
    output.push_str("/// Get BSD syscall name by number\n");
    output.push_str("pub fn bsd_syscall_name(num: u32) -> Option<&\'static str> {\n");
    output.push_str("    BSD_SYSCALL_TABLE.iter()\n");
    output.push_str("        .find(|e| e.number == num)\n");
    output.push_str("        .map(|e| e.name)\n");
    output.push_str("}\n");
    output.push('\n');

    // Utility function: get mach trap name by number
    output.push_str("/// Get Mach trap name by number\n");
    output.push_str("pub fn mach_trap_name(num: i32) -> Option<&\'static str> {\n");
    output.push_str("    MACH_TRAP_TABLE.iter()\n");
    output.push_str("        .find(|e| e.number == num)\n");
    output.push_str("        .map(|e| e.name)\n");
    output.push_str("}\n");

    output
}

/// Sanitize a syscall name for use as a Rust constant
///
/// Examples:
///   "exit" -> "exit"
///   "open_nocancel" -> "open_nocancel"
///   "lseek$NOCANCEL" -> "lseek_NOCANCEL"
fn sanitize_syscall_name(name: &str) -> String {
    let name = name.strip_prefix("sys_").unwrap_or(name);
    name.replace("$", "_")
        .replace(".", "_")
        .replace("-", "_")
}

/// Sanitize a Mach trap name for use as a Rust constant
///
/// Examples:
///   "mach_msg_trap" -> "mach_msg_trap"
///   "kernel_rpc_mach_reply_port" -> "kernel_rpc_mach_reply_port"
fn sanitize_mach_trap_name(name: &str) -> String {
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prototype_simple() {
        let (name, nargs) = parse_prototype("void exit(int rval)").unwrap();
        assert_eq!(name, "exit");
        assert_eq!(nargs, 1);
    }

    #[test]
    fn test_parse_prototype_no_args() {
        let (name, nargs) = parse_prototype("pid_t fork(void)").unwrap();
        assert_eq!(name, "fork");
        assert_eq!(nargs, 0);
    }

    #[test]
    fn test_parse_prototype_multiple_args() {
        let (name, nargs) = parse_prototype("ssize_t read(int fd, user_addr_t cbuf, user_size_t nbyte)").unwrap();
        assert_eq!(name, "read");
        assert_eq!(nargs, 3);
    }

    #[test]
    fn test_parse_prototype_void_args() {
        let (name, nargs) = parse_prototype("int getpid(void)").unwrap();
        assert_eq!(name, "getpid");
        assert_eq!(nargs, 0);
    }

    #[test]
    fn test_parse_prototype_complex() {
        let (name, nargs) = parse_prototype("int open(user_addr_t path, int flags, int mode)").unwrap();
        assert_eq!(name, "open");
        assert_eq!(nargs, 3);
    }

    #[test]
    fn test_sanitize_syscall_name() {
        assert_eq!(sanitize_syscall_name("exit"), "exit");
        assert_eq!(sanitize_syscall_name("open_nocancel"), "open_nocancel");
        assert_eq!(sanitize_syscall_name("lseek$NOCANCEL"), "lseek_NOCANCEL");
    }

    #[test]
    fn test_is_movz() {
        assert!(is_movz(0xd2800030));
        assert!(!is_movz(0xf2a04010));
        assert!(!is_movz(0xd4001001));
    }

    #[test]
    fn test_is_movk() {
        assert!(is_movk(0xf2a04010));
        assert!(!is_movk(0xd2800030));
        assert!(!is_movk(0xd4001001));
    }

    #[test]
    fn test_is_svc() {
        assert!(is_svc(0xd4001001));
        assert!(!is_svc(0xd2800030));
    }

    #[test]
    fn test_decode_mov_imm16() {
        assert_eq!(decode_mov_imm16(0xd2800030), Some(1));
        assert_eq!(decode_mov_imm16(0xf2a04010), Some(0x200));
    }

    #[test]
    fn test_decode_mov_hw() {
        assert_eq!(decode_mov_hw(0xd2800030), Some(0));
        assert_eq!(decode_mov_hw(0xf2a04010), Some(16));
    }

    #[test]
    fn test_extract_syscall_number_movz_movk_svc() {
        let instructions = vec![0xd2800030, 0xf2a04010, 0xd4001001];
        assert_eq!(extract_syscall_number(&instructions), Some(0x2000001));
    }

    #[test]
    fn test_extract_syscall_number_movz_only_svc() {
        let instructions = vec![0xd2800070, 0xd4001001, 0x00000000];
        assert_eq!(extract_syscall_number(&instructions), Some(3));
    }

    #[test]
    fn test_extract_syscall_number_no_movz() {
        let instructions = vec![0xd4000001, 0x00000000, 0x00000000];
        assert_eq!(extract_syscall_number(&instructions), None);
    }

    #[test]
    fn test_extract_syscall_number_too_short() {
        let instructions = vec![0xd2800020, 0xd4000001];
        assert_eq!(extract_syscall_number(&instructions), None);
    }

    #[test]
    fn test_read_struct_mach_header() {
        let bytes = [
            0xcf, 0xfa, 0xed, 0xfe,
            0x0c, 0x00, 0x00, 0x01,
            0x00, 0x00, 0x00, 0x00,
            0x06, 0x00, 0x00, 0x00,
            0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
        ];
        let header = read_struct::<MachHeader64>(&bytes).unwrap();
        assert_eq!(header.magic, MH_MAGIC_64);
        assert_eq!(header.cputype, CPU_TYPE_ARM64);
        assert_eq!(header.filetype, MH_DYLIB);
    }

    #[test]
    fn test_segname_to_string() {
        let mut segname = [0u8; 16];
        segname[..6].copy_from_slice(b"__TEXT");
        assert_eq!(segname_to_string(&segname), "__TEXT");

        let mut sectname = [0u8; 16];
        sectname[..6].copy_from_slice(b"__text");
        assert_eq!(segname_to_string(&sectname), "__text");
    }
}
