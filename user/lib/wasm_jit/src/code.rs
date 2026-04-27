use alloc::vec::Vec;

use crate::control::Fixup;

pub struct CodeBuffer {
    pub bytes: Vec<u8>,
    pub fixups: Vec<Fixup>,
}

impl CodeBuffer {
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            fixups: Vec::new(),
        }
    }

    pub fn offset(&self) -> u32 {
        self.bytes.len() as u32
    }

    pub fn emit_byte(&mut self, b: u8) {
        self.bytes.push(b);
    }

    pub fn emit_u32(&mut self, v: u32) {
        self.bytes.extend_from_slice(&v.to_le_bytes());
    }

    pub fn emit_bytes(&mut self, bs: &[u8]) {
        self.bytes.extend_from_slice(bs);
    }

    pub fn patch_u32_at(&mut self, offset: u32, value: u32) {
        let o = offset as usize;
        if o + 4 <= self.bytes.len() {
            self.bytes[o..o + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    pub fn add_fixup(&mut self, fixup: Fixup) {
        self.fixups.push(fixup);
    }
}

pub struct ExecutableSlab {
    pub ptr: *mut u8,
    pub len: usize,
}

impl ExecutableSlab {
    pub fn empty() -> Self {
        Self {
            ptr: core::ptr::null_mut(),
            len: 0,
        }
    }
}

unsafe impl Send for ExecutableSlab {}
unsafe impl Sync for ExecutableSlab {}
