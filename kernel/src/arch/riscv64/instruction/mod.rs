use core::{arch::asm, ptr::read_unaligned};

pub mod sbi;

pub fn idle() {
    loop {
        unsafe {
            asm!("wfi", options(nostack));
        }
    }
}

/// RISC-V environment call (ecall) with proper register preservation
/// using clobber_abi to handle register preservation automatically
#[inline(never)]
#[unsafe(no_mangle)]
pub fn ecall(a0: usize, a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize, a7: usize) -> usize {
    let ret: usize;
    
    unsafe {
        asm!(
            "ecall",
            inout("a0") a0 => ret,
            inout("a1") a1 => _,
            inout("a2") a2 => _,
            inout("a3") a3 => _,
            inout("a4") a4 => _,
            inout("a5") a5 => _,
            inout("a6") a6 => _,
            inout("a7") a7 => _,
            clobber_abi("C"),
            options(nostack),
        );
    }

    ret
}

/// Represents a RISC-V instruction.
/// This struct is used to encapsulate the raw instruction data
/// and provides methods to create an instruction from raw bytes or a usize value.
/// 
pub struct Instruction {
    pub raw: u32,
}

impl Instruction {
    pub fn new(raw: usize) -> Self {
        Self { raw: raw as u32 }
    }

    pub fn fetch(addr: usize) -> Self {
        Instruction {
            raw: unsafe { read_unaligned(addr as *const u32) },
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.len() < 4 {
            panic!("Instruction bytes must be at least 4 bytes long");
        }
        let raw = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        Self { raw }
    }

    pub fn len(&self) -> usize {
        if (self.raw & 0b11) == 0b11 {
            4 // 32-bit instruction
        } else {
            2 // 16-bit instruction
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_instruction_from_bytes() {
        let bytes = [0x13, 0x00, 0x00, 0x00]; // Example instruction bytes
        let instruction = Instruction::from_bytes(&bytes);
        assert_eq!(instruction.raw, 0x00000013);
        assert_eq!(instruction.len(), 4);
    }

    #[test_case]
    fn test_instruction_new() {
        let instruction = Instruction::new(0x00000013);
        assert_eq!(instruction.raw, 0x00000013);
        assert_eq!(instruction.len(), 4);
    }

    #[test_case]
    fn test_instruction_len() {
        let instruction_32 = Instruction::new(0x00000013); // 32-bit instruction
        assert_eq!(instruction_32.len(), 4);

        let instruction_16 = Instruction::new(0x00000004); // 16-bit instruction
        assert_eq!(instruction_16.len(), 2);
    }
}