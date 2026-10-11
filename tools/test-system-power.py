#!/usr/bin/env python3
"""Host regression checks for production PSCI selection and stemd power control.

The temporary Cargo harness imports the actual production modules. Hardware
instructions remain target-only; run the normal AArch64 build and platform QA
as well. No source or dependency cache is copied or patched.
"""
from pathlib import Path
import json
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MODULES = {
    "psci": "kernel/src/arch/aarch64/psci.rs",
    "system_power": "kernel/src/system_power.rs",
    "power_request": "user/std-bin/src/power_request.rs",
    "stemd_power": "user/std-bin/src/stemd/power.rs",
}

with tempfile.TemporaryDirectory(prefix="scarlet-system-power-") as temporary:
    project = Path(temporary)
    (project / "src").mkdir()
    (project / "Cargo.toml").write_text('''[package]
name = "scarlet-system-power-host-tests"
version = "0.0.0"
edition = "2024"
[dependencies]
fdt = "=0.1.5"
[workspace]
''')
    (project / "src/lib.rs").write_text('''
extern crate alloc;
pub use std::println;
// Only platform plumbing is stubbed; policy tests import production dispatch.
mod sync { pub type Once<T> = std::sync::OnceLock<T>; }
mod arch {
    pub fn firmware_system_power(_: crate::system_power::Action)
        -> crate::system_power::PowerResult { panic!("host cannot call firmware") }
    pub mod instruction { pub fn idle() -> ! { panic!("host cannot park CPU") } }
}
''' + "\n".join(
        f"#[path = {json.dumps(str(ROOT / relative))}]\nmod {name};"
        for name, relative in MODULES.items()) + "\n")
    subprocess.run(["cargo", "test", "--manifest-path", str(project / "Cargo.toml")],
                   cwd=project, check=True)
