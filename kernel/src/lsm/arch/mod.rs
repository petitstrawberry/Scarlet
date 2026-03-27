use crate::lsm::elf::RelocObject;

pub mod aarch64;
pub mod riscv64;

pub fn apply_relocations(
    object: &RelocObject,
    section_bases: &[(usize, usize)],
    symbol_resolver: &dyn Fn(&str) -> Option<usize>,
) -> Result<(), &'static str> {
    #[cfg(target_arch = "riscv64")]
    {
        return riscv64::apply_relocations(object, section_bases, symbol_resolver);
    }

    #[cfg(target_arch = "aarch64")]
    {
        return aarch64::apply_relocations(object, section_bases, symbol_resolver);
    }

    #[allow(unreachable_code)]
    {
        let _ = (object, section_bases, symbol_resolver);
        Err("Unsupported target architecture for LSM relocations")
    }
}
