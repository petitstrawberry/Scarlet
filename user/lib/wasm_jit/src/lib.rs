#![no_std]

extern crate alloc;

pub mod arch;
pub mod code;
pub mod compiler;
pub mod control;
pub mod engine;
pub mod frame;
pub mod helpers;
pub mod module;
pub mod runtime;
pub mod wasi;

use alloc::{boxed::Box, string::String, vec::Vec};

use crate::runtime::VmContext;

pub type RawValue = u64;

pub type CompiledFn = unsafe extern "C" fn(*mut VmContext, *mut RawValue) -> RawValue;

pub struct CompiledModule {
    pub code: code::ExecutableSlab,
    pub functions: Box<[FunctionEntry]>,
    pub exports: Box<[ExportEntry]>,
    pub imported_funcs: Box<[ImportedFuncEntry]>,
    pub data_segments: Box<[DataSegment]>,
    pub globals: Box<[GlobalEntry]>,
    pub imported_global_count: u32,
    pub table: Box<[u32]>,
    /// Minimum memory pages declared by the wasm memory section.
    pub min_memory_pages: u32,
}

pub struct ImportedFuncEntry {
    pub module: String,
    pub name: String,
    pub type_index: u32,
}

pub struct DataSegment {
    pub offset: u32,
    pub data: Vec<u8>,
}

pub struct GlobalEntry {
    pub value: u64,
    pub mutable: bool,
}

impl Clone for GlobalEntry {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            mutable: self.mutable,
        }
    }
}

impl Clone for DataSegment {
    fn clone(&self) -> Self {
        Self {
            offset: self.offset,
            data: self.data.clone(),
        }
    }
}

impl CompiledModule {
    pub fn init_memory(&self, memory: &mut [u8]) {
        for seg in &self.data_segments {
            let end = seg.offset as usize + seg.data.len();
            if end <= memory.len() {
                memory[seg.offset as usize..end].copy_from_slice(&seg.data);
            }
        }
    }
}

pub struct FunctionEntry {
    pub code: CompiledFn,
    pub frame_slots: u16,
    pub param_count: u16,
    pub local_count: u16,
    pub max_stack: u16,
}

pub struct ExportEntry {
    pub name: String,
    pub func_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum TrapCode {
    None,
    MemoryOutOfBounds,
    DivisionByZero,
    Unreachable,
    BadCall,
    StackOverflow,
    ProcExit,
}

#[derive(Debug)]
pub enum CompileError {
    InvalidWasm(&'static str),
    UnsupportedFeature(&'static str),
    CodeGen(&'static str),
    FuncCompileError {
        func_index: u32,
        inner: &'static str,
    },
}
