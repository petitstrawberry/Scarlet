use crate::environment::PAGE_SIZE;
use scarlet_abi::data_model::AbiDataModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserCopyError {
    NullPointer,
    AddressOverflow,
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
    check_range(user_addr, dst.len())?;

    let mut copied = 0usize;
    while copied < dst.len() {
        let current_vaddr = user_addr + copied;
        let page_off = current_vaddr & (PAGE_SIZE - 1);
        let chunk_len = core::cmp::min(dst.len() - copied, PAGE_SIZE - page_off);
        let kaddr = task
            .vm_manager
            .translate_to_kva(current_vaddr)
            .ok_or(UserCopyError::TranslationError)?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                kaddr as *const u8,
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
    check_range(user_addr, src.len())?;

    let mut copied = 0usize;
    while copied < src.len() {
        let current_vaddr = user_addr + copied;
        let page_off = current_vaddr & (PAGE_SIZE - 1);
        let chunk_len = core::cmp::min(src.len() - copied, PAGE_SIZE - page_off);
        let kaddr = task
            .vm_manager
            .translate_to_kva_for_write(current_vaddr)
            .ok_or(UserCopyError::TranslationError)?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                src[copied..copied + chunk_len].as_ptr(),
                kaddr as *mut u8,
                chunk_len,
            );
        }

        copied += chunk_len;
    }

    Ok(())
}

// Reject arithmetic wrap before any page is translated or copied. This checks
// representation only; each page still needs the VM's access checks, and a
// later mapping failure can still leave an already-copied prefix.
fn check_range(address: usize, len: usize) -> Result<(), UserCopyError> {
    AbiDataModel::NATIVE
        .user_address(address as u64)
        .and_then(|address| address.check_range(len as u64))
        .map_err(|_| UserCopyError::AddressOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn wrapping_ranges_fail_before_copying() {
        let task = crate::task::new_user_task("usercopy-range".into(), 1);
        let mut bytes = [0xa5; 2];
        assert_eq!(
            copy_from_user(&task, usize::MAX, &mut bytes),
            Err(UserCopyError::AddressOverflow)
        );
        assert_eq!(bytes, [0xa5; 2]);
        assert_eq!(
            copy_to_user(&task, usize::MAX, &bytes),
            Err(UserCopyError::AddressOverflow)
        );
    }

    #[test_case]
    fn empty_and_null_copy_contract() {
        let task = crate::task::new_user_task("usercopy-empty".into(), 1);
        assert_eq!(copy_from_user(&task, 0, &mut []), Ok(()));
        assert_eq!(copy_to_user(&task, 0, &[]), Ok(()));
        assert_eq!(
            copy_from_user(&task, 0, &mut [0]),
            Err(UserCopyError::NullPointer)
        );
        assert_eq!(
            copy_to_user(&task, 0, &[0]),
            Err(UserCopyError::NullPointer)
        );
        assert_eq!(check_range(usize::MAX, 1), Ok(()));
    }
}
