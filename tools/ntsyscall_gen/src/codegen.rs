use crate::scanner::SyscallEntry;

pub fn generate_syscall_table_rs(
    entries: &[SyscallEntry],
    version: Option<String>,
    regenerate_cmd: String,
    source_path: String,
) -> String {
    let version = version.unwrap_or_else(|| "unknown".to_string());

    let mut by_name = entries.to_vec();
    by_name.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::new();
    out.push_str("// Generated from ");
    out.push_str(&source_path);
    out.push('\n');
    out.push_str("// Regenerate: ");
    out.push_str(&regenerate_cmd);
    out.push_str("\n//\n");
    out.push_str("// This file is auto-generated. DO NOT EDIT MANUALLY.\n\n");
    out.push_str("//! NT syscall table extracted from ntdll.dll\n");
    out.push_str("//! Contains syscall numbers for ARM64 Windows binaries.\n\n");
    out.push_str("/// Source ntdll.dll version info\n");
    out.push_str("pub const NTDLL_VERSION: &str = \"");
    out.push_str(&escape_rust_str(&version));
    out.push_str("\";\n\n");
    out.push_str("/// A single NT syscall entry\n");
    out.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n");
    out.push_str("pub struct NtSyscallEntry {\n");
    out.push_str("    /// Syscall number (from SVC immediate)\n");
    out.push_str("    pub number: u16,\n");
    out.push_str("    /// Function name (e.g., \"NtCreateFile\")\n");
    out.push_str("    pub name: &'static str,\n");
    out.push_str("}\n\n");

    out.push_str("/// Complete syscall table sorted by syscall number\n");
    out.push_str("pub const NT_SYSCALL_TABLE: &[NtSyscallEntry] = &[\n");
    for entry in entries {
        out.push_str("    NtSyscallEntry { number: 0x");
        out.push_str(&format!("{:04X}", entry.number));
        out.push_str(", name: \"");
        out.push_str(&escape_rust_str(&entry.name));
        out.push_str("\" },\n");
    }
    out.push_str("];\n\n");

    out.push_str("const _NT_SYSCALLS_BY_NAME: &[NtSyscallEntry] = &[\n");
    for entry in &by_name {
        out.push_str("    NtSyscallEntry { number: 0x");
        out.push_str(&format!("{:04X}", entry.number));
        out.push_str(", name: \"");
        out.push_str(&escape_rust_str(&entry.name));
        out.push_str("\" },\n");
    }
    out.push_str("];\n\n");

    out.push_str("/// Look up syscall number by name\n");
    out.push_str("pub fn lookup_by_name(name: &str) -> Option<u16> {\n");
    out.push_str("    let mut left = 0usize;\n");
    out.push_str("    let mut right = _NT_SYSCALLS_BY_NAME.len();\n");
    out.push_str("    while left < right {\n");
    out.push_str("        let mid = left + ((right - left) / 2);\n");
    out.push_str("        match _NT_SYSCALLS_BY_NAME[mid].name.cmp(name) {\n");
    out.push_str("            core::cmp::Ordering::Less => left = mid + 1,\n");
    out.push_str("            core::cmp::Ordering::Greater => right = mid,\n");
    out.push_str("            core::cmp::Ordering::Equal => return Some(_NT_SYSCALLS_BY_NAME[mid].number),\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("    None\n");
    out.push_str("}\n\n");

    out.push_str("/// Look up syscall name by number\n");
    out.push_str("pub fn lookup_by_number(number: u16) -> Option<&'static str> {\n");
    out.push_str("    let mut left = 0usize;\n");
    out.push_str("    let mut right = NT_SYSCALL_TABLE.len();\n");
    out.push_str("    while left < right {\n");
    out.push_str("        let mid = left + ((right - left) / 2);\n");
    out.push_str("        let value = NT_SYSCALL_TABLE[mid].number;\n");
    out.push_str("        if value < number {\n");
    out.push_str("            left = mid + 1;\n");
    out.push_str("        } else if value > number {\n");
    out.push_str("            right = mid;\n");
    out.push_str("        } else {\n");
    out.push_str("            return Some(NT_SYSCALL_TABLE[mid].name);\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("    None\n");
    out.push_str("}\n");
    out
}

pub fn generate_reference_table(
    entries: &[SyscallEntry],
    version: Option<String>,
    source_path: String,
) -> String {
    let version = version.unwrap_or_else(|| "unknown".to_string());

    let mut out = String::new();
    out.push_str("# NT Syscall Reference Table (ARM64)\n\n");
    out.push_str("- Source file: `");
    out.push_str(&source_path);
    out.push_str("`\n");
    out.push_str("- ntdll version: `");
    out.push_str(&version);
    out.push_str("`\n");
    out.push_str("- Entry count: `");
    out.push_str(&entries.len().to_string());
    out.push_str("`\n\n");
    out.push_str("This table is intended for verification against public references such as hfiref0x/SyscallTables.\n\n");

    out.push_str("| Number (hex) | Number (dec) | Name | RVA |\n");
    out.push_str("|---:|---:|---|---:|\n");
    for e in entries {
        out.push_str("| 0x");
        out.push_str(&format!("{:04X}", e.number));
        out.push_str(" | ");
        out.push_str(&e.number.to_string());
        out.push_str(" | `");
        out.push_str(&e.name);
        out.push_str("` | 0x");
        out.push_str(&format!("{:08X}", e.rva));
        out.push_str(" |\n");
    }
    out
}

fn escape_rust_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}
