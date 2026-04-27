use crate::runtime::VmContext;
use crate::{CompileError, CompiledModule, RawValue, TrapCode};

/// Type for executable memory allocation functions.
///
/// The allocator takes a size in bytes and returns a pointer to
/// RWX (read-write-execute) memory, or null on failure.
pub type ExecMemAllocator = fn(usize) -> *mut u8;

/// Default allocator using standard heap (not executable on platforms with W^X).
fn default_exec_allocator(size: usize) -> *mut u8 {
    let mut slab = alloc::vec![0u8; size].into_boxed_slice();
    let ptr = slab.as_mut_ptr();
    core::mem::forget(slab);
    ptr
}

static mut EXEC_ALLOCATOR: ExecMemAllocator = default_exec_allocator;

/// Set the executable memory allocator.
///
/// Must be called before `compile_module()`. On platforms with W^X
/// enforcement (e.g. Scarlet OS), pass a function that allocates
/// memory with execute permission (e.g. via mmap with PROT_EXEC).
pub fn set_exec_allocator(alloc: ExecMemAllocator) {
    // SAFETY: single-threaded init before any compile_module calls
    unsafe {
        EXEC_ALLOCATOR = alloc;
    }
}

pub fn compile_module(wasm: &[u8]) -> Result<CompiledModule, CompileError> {
    #[cfg(target_arch = "riscv64")]
    let mut compiler =
        crate::compiler::ModuleCompiler::new(crate::arch::riscv64::Riscv64Backend::new());
    #[cfg(target_arch = "aarch64")]
    let mut compiler =
        crate::compiler::ModuleCompiler::new(crate::arch::aarch64::Aarch64Backend::new());

    compiler.compile(wasm)
}

/// Allocate executable memory using the configured allocator.
pub(crate) fn alloc_exec_memory(size: usize) -> *mut u8 {
    // SAFETY: called during compilation, after set_exec_allocator if configured
    unsafe { EXEC_ALLOCATOR(size) }
}

pub unsafe fn invoke_export(
    module: &CompiledModule,
    ctx: *mut VmContext,
    name: &str,
    args: &[RawValue],
) -> Result<RawValue, TrapCode> {
    let entry = module.exports.iter().find(|e| e.name == name);
    match entry {
        Some(export) => {
            let func = &module.functions[export.func_index as usize];
            let mut frame = alloc::vec![0u64; func.frame_slots as usize];
            for (i, arg) in args.iter().enumerate() {
                if i < func.param_count as usize {
                    frame[i] = *arg;
                }
            }
            unsafe {
                let result = (func.code)(ctx, frame.as_mut_ptr());
                if (*ctx).trap != TrapCode::None {
                    Err((*ctx).trap)
                } else {
                    Ok(result)
                }
            }
        }
        None => Err(TrapCode::BadCall),
    }
}
