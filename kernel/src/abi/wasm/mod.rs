//! WebAssembly ABI module.
//!
//! This module detects WebAssembly binaries (`.wasm` files) and delegates
//! their execution to a userland Wasm runtime via the Runtime Delegation
//! mechanism. The kernel does not execute Wasm directly — instead it
//! launches the configured runtime binary which handles Wasm execution
//! in userspace.
//!
//! # Execution Flow
//!
//! 1. `TransparentExecutor` opens a binary file
//! 2. `AbiRegistry::detect_best_abi()` tries all registered ABIs
//! 3. `WasmAbi::can_execute_binary()` detects Wasm magic bytes → confidence 80
//! 4. `WasmAbi::get_runtime_config()` returns `RuntimeConfig` pointing to
//!    `/system/scarlet/bin/wasm-runtime`
//! 5. `TransparentExecutor::execute_via_runtime()` launches the runtime
//!    with the `.wasm` file as an argument
//!
//! # Runtime
//!
//! The userspace Wasm runtime (`wasm-runtime`) is a Scarlet native binary
//! that embeds a Wasm engine and provides WASI preview1 support bridged
//! to Scarlet syscalls.

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};

use crate::abi::{AbiModule, RuntimeConfig};
use crate::{fs::SeekFrom, late_initcall, register_abi};

/// WebAssembly ABI module.
///
/// Detects Wasm binaries and delegates execution to the userspace runtime.
/// This ABI module has no syscall handling — all Wasm execution happens in
/// the userspace runtime process.
#[derive(Debug, Default, Clone)]
pub struct WasmAbi;

/// Wasm binary magic bytes: `\0asm` (0x00 0x61 0x73 0x6D)
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

/// Expected Wasm version (1.0): 0x01 0x00 0x00 0x00
const WASM_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Path to the userspace Wasm runtime binary
const WASM_RUNTIME_PATH: &str = "/scarlet/system/scarlet/bin/wasm-runtime";

impl AbiModule for WasmAbi {
    fn name() -> &'static str
    where
        Self: Sized,
    {
        "wasm"
    }

    fn get_name(&self) -> String {
        "wasm".to_string()
    }

    fn clone_boxed(&self) -> Box<dyn AbiModule + Send + Sync> {
        Box::new(self.clone())
    }

    fn handle_syscall(
        &mut self,
        _trapframe: &mut crate::arch::Trapframe,
    ) -> Result<usize, &'static str> {
        Err("Wasm ABI has no syscall support")
    }

    /// Detect Wasm binaries by magic bytes.
    ///
    /// # Scoring
    ///
    /// - 50: Valid Wasm magic + version header
    /// - 30: Valid Wasm magic but version check failed (still likely Wasm)
    /// - 15: `.wasm` file extension hint (only if magic bytes unreadable)
    /// - 40: ABI inheritance bonus (if current task is already wasm ABI)
    fn can_execute_binary(
        &self,
        file_object: &crate::object::KernelObject,
        file_path: &str,
        current_abi: Option<&(dyn AbiModule + Send + Sync)>,
    ) -> Option<u8> {
        let mut confidence: u8 = 0;

        // Stage 1: Check Wasm magic bytes
        if let Some(file_obj) = file_object.as_file() {
            let mut header = [0u8; 8];
            file_obj.seek(SeekFrom::Start(0)).ok();
            match file_obj.read(&mut header) {
                Ok(bytes_read) if bytes_read >= 4 => {
                    if header[..4] == WASM_MAGIC {
                        if bytes_read >= 8 && header[4..] == WASM_VERSION {
                            confidence += 50;
                        } else {
                            confidence += 30;
                        }
                    } else {
                        return None;
                    }
                }
                _ => {
                    if file_path.ends_with(".wasm") {
                        confidence += 15;
                    } else {
                        return None;
                    }
                }
            }

            file_obj.seek(SeekFrom::Start(0)).ok();
        } else {
            return None;
        }

        // Stage 2: File path hints
        if file_path.ends_with(".wasm") {
            confidence = confidence.saturating_add(10);
        }

        // Stage 3: ABI inheritance bonus
        if let Some(abi) = current_abi {
            if abi.get_name() == self.get_name() {
                confidence = confidence.saturating_add(40);
            }
        }

        Some(confidence.min(100))
    }

    fn get_runtime_config(
        &self,
        _file_object: &crate::object::KernelObject,
        _file_path: &str,
    ) -> Option<RuntimeConfig> {
        Some(RuntimeConfig {
            runtime_path: WASM_RUNTIME_PATH.to_string(),
            runtime_abi: None,
            runtime_args: Vec::new(),
        })
    }

    fn execute_binary(
        &self,
        _file_object: &crate::object::KernelObject,
        _argv: &[&str],
        _envp: &[&str],
        _task: &crate::task::Task,
        _trapframe: &mut crate::arch::Trapframe,
    ) -> Result<(), &'static str> {
        Err("Wasm binaries must be executed via runtime delegation")
    }

    fn get_default_cwd(&self) -> &str {
        "/"
    }
}

fn register_wasm_abi() {
    register_abi!(WasmAbi);
}

late_initcall!(register_wasm_abi);
