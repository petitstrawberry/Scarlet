use crate::environment::PAGE_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCopyError {
    NullPointer,
    TranslationError,
}

pub fn copy_from_user(
    task: &crate::task::Task,
    user_addr: usize,
    dst: &mut [u8],
) -> Result<(), UserCopyError> {
    if dst.is_empty() {
        return Ok(());
    }
    if user_addr == 0 {
        return Err(UserCopyError::NullPointer);
    }

    let mut copied = 0usize;
    while copied < dst.len() {
        let current_vaddr = user_addr + copied;
        let page_off = current_vaddr & (PAGE_SIZE - 1);
        let chunk_len = core::cmp::min(dst.len() - copied, PAGE_SIZE - page_off);
        let paddr = task
            .vm_manager
            .translate_vaddr(current_vaddr)
            .ok_or(UserCopyError::TranslationError)?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                paddr as *const u8,
                dst[copied..copied + chunk_len].as_mut_ptr(),
                chunk_len,
            );
        }

        copied += chunk_len;
    }

    Ok(())
}

pub fn copy_to_user(
    task: &crate::task::Task,
    user_addr: usize,
    src: &[u8],
) -> Result<(), UserCopyError> {
    if src.is_empty() {
        return Ok(());
    }
    if user_addr == 0 {
        return Err(UserCopyError::NullPointer);
    }

    let mut copied = 0usize;
    while copied < src.len() {
        let current_vaddr = user_addr + copied;
        let page_off = current_vaddr & (PAGE_SIZE - 1);
        let chunk_len = core::cmp::min(src.len() - copied, PAGE_SIZE - page_off);
        let paddr = task
            .vm_manager
            .translate_vaddr(current_vaddr)
            .ok_or(UserCopyError::TranslationError)?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                src[copied..copied + chunk_len].as_ptr(),
                paddr as *mut u8,
                chunk_len,
            );
        }

        copied += chunk_len;
    }

    Ok(())
}
