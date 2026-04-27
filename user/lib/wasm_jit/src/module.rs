use alloc::{string::String, vec::Vec};

pub struct ModuleInfo {
    pub type_section: Vec<FuncType>,
    pub function_count: u32,
    pub exports: Vec<(String, u32)>,
    pub memories: Vec<MemoryInfo>,
}

#[derive(Clone)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
}

pub struct MemoryInfo {
    pub initial_pages: u32,
    pub max_pages: Option<u32>,
}

impl ModuleInfo {
    pub fn param_count(&self, func_idx: u32) -> u16 {
        let type_idx = func_idx as usize;
        if type_idx < self.type_section.len() {
            self.type_section[type_idx].params.len() as u16
        } else {
            0
        }
    }

    pub fn result_count(&self, func_idx: u32) -> u16 {
        let type_idx = func_idx as usize;
        if type_idx < self.type_section.len() {
            self.type_section[type_idx].results.len() as u16
        } else {
            0
        }
    }
}
